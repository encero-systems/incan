//! Sealed runtime-foundation authority for Cargo-free Oven publication.
//!
//! A runtime foundation is an SDK-provided, immutable description of the third-party Rust artifacts and the small
//! compiler-owned source layer that Oven may rebuild above them. It does not resolve packages, inspect a Cargo
//! manifest, or scan a target directory. Instead it validates a producer-selected Rust facet graph and projects its
//! declared prebuilt dependency edges for the direct-Rustc publisher.

#![allow(
    dead_code,
    reason = "the runtime source publisher loads this sealed authority in the next wiring slice"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{
    OvenMaterializedRustFacetEnvironmentValue, OvenMaterializedRustFacetGraph, OvenRustcArtifactExtern,
    OvenRustcArtifactManifest, OvenRustcArtifactPlan, OvenRustcError, OvenSelectedRustFacetCrateKind,
    OvenSelectedRustFacetDomain, OvenSelectedRustFacetEnvironmentValue, OvenSelectedRustFacetGeneratedInput,
    OvenSelectedRustFacetGraph, OvenSelectedRustFacetOwnerKind, OvenSelectedRustFacetOwnerRoot,
    OvenSelectedRustFacetPath, OvenSelectedRustFacetSourceKind, OvenSelectedRustFacetSourceMember,
    OvenSelectedRustFacetSupplementalSourceMembers, OvenSelectedRustFacetUnit, OvenSelectedRustFacetUnitRole,
    ValidatedOvenSelectedRustFacetGraph, canonical_directory, digest_bytes,
    materialize_selected_rust_facet_graph_with_supplemental_source_members, normalized_relative_path, safe_path,
    selected_graph_source_digest, verified_regular_file,
};

/// Wire schema for the first sealed compiler-runtime foundation.
pub(crate) const OVEN_RUNTIME_FOUNDATION_SCHEMA_VERSION: u32 = 1;

/// Wire schema for a release-owned runtime-foundation asset.
pub(crate) const OVEN_RUNTIME_FOUNDATION_ASSET_SCHEMA_VERSION: u32 = 3;

/// Canonical descriptor filename retained at the root of one installed runtime-foundation asset.
pub(crate) const OVEN_RUNTIME_FOUNDATION_ASSET_FILENAME: &str = "foundation.json";

/// A versioned SDK-owned authority for one direct-Rustc compiler-runtime closure.
///
/// selected_graph is the one source/unit/dependency projection consumed by both direct Rustc and inspection. The
/// foundation contributes only immutable source and artifact facts plus the policy declaring which units are already
/// prebuilt and which compiler-owned units may be rebuilt. It never rediscovers those relationships from Cargo data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OvenRuntimeFoundation {
    /// Runtime-foundation wire schema.
    pub(crate) schema_version: u32,
    /// Exact direct-Rustc compiler closure identity that owns every compiled-unit identity below this foundation.
    pub(crate) compiler_closure_digest: String,
    /// Immutable selected-graph owner whose root contains this foundation's sealed source and artifact catalogue.
    pub(crate) artifact_owner: String,
    /// Complete sealed artifact/source catalogue retained by the SDK foundation.
    pub(crate) artifacts: OvenRustcArtifactManifest,
    /// One selected Rust source graph produced by the foundation publisher.
    pub(crate) selected_graph: OvenSelectedRustFacetGraph,
    /// One explicit execution policy for every selected graph unit.
    pub(crate) units: Vec<OvenRuntimeFoundationUnit>,
}

/// One versioned release asset carrying a sealed runtime foundation and complete provider-output evidence.
///
/// `foundation_identity` names this authority record, including selected source coordinates. It deliberately differs
/// from individual compiled-unit identities: changing an observed source authority can replace this asset while
/// unchanged compiler-visible units still reuse their content-addressed outputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OvenRuntimeFoundationAsset {
    /// Runtime-foundation asset wire schema.
    pub(crate) schema_version: u32,
    /// Canonical digest of this asset's authority and provider-output facts.
    pub(crate) foundation_identity: String,
    /// Complete selected source/artifact policy for the installed foundation.
    pub(crate) foundation: OvenRuntimeFoundation,
    /// One exhaustive provider-output state for every selected source unit.
    pub(crate) providers: Vec<OvenRuntimeFoundationProviderRecord>,
}

/// Captured or explicitly unsupported provider facts for one selected runtime unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OvenRuntimeFoundationProviderRecord {
    /// Selected-source identity of the unit this provider state describes.
    pub(crate) selected_identity: String,
    /// Explicit producer-selected declaration of whether this unit owns a build-script execution.
    pub(crate) declaration: OvenRuntimeFoundationProviderDeclaration,
    /// Exhaustive state of the unit's build-time provider effects.
    pub(crate) state: OvenRuntimeFoundationProviderState,
}

/// One explicit provider declaration supplied alongside the selected Rust unit.
///
/// This is deliberately not inferred from generated output. A build script can emit only cfg or environment facts,
/// so an empty generated-input list does not establish that no provider ran. The declaration names only source
/// members and host-domain units that the existing selected graph already owns; it cannot resolve another package
/// graph or discover a manifest at publication time. Its package source is deliberately separate from the
/// compiler-visible unit source catalogue so a manifest or provider-only byte cannot invalidate an otherwise
/// unchanged Rust unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OvenRuntimeFoundationProviderPackageSource {
    /// Retained owner-relative package root, which may sit above the compiler-visible unit root.
    pub(crate) root: OvenSelectedRustFacetPath,
    /// Exact package manifest whose checked source declaration produced this provider outcome.
    pub(crate) manifest: OvenSelectedRustFacetSourceMember,
    /// Complete package-root inventory apart from `manifest`, retained for physical verification and, when needed,
    /// build-script read authority.
    pub(crate) members: Vec<OvenSelectedRustFacetSourceMember>,
}

/// One explicit provider declaration supplied alongside the selected Rust unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum OvenRuntimeFoundationProviderDeclaration {
    /// The original selected manifest declared no build-script unit, with its package evidence still retained.
    NoBuildScript {
        /// Package evidence that binds this negative declaration to exact source bytes.
        package: OvenRuntimeFoundationProviderPackageSource,
    },
    /// One declared provider source member is compiled and executed as a host build script.
    BuildScript {
        /// Exact package-root evidence for the build script.
        ///
        /// The package inventory is physical authority for the build script, not a second selected Rust unit or a
        /// compiler-visible input to the owning unit. It may include the compiler tree at package-relative paths;
        /// those records remain absent from the compiled-unit input.
        package: OvenRuntimeFoundationProviderPackageSource,
        /// Portable path of the exact package source member used as the script entrypoint.
        entrypoint: String,
        /// Digest of that exact package source member.
        digest: String,
        /// Rust edition selected for the package and its build script.
        edition: String,
        /// Explicit host-domain build dependencies; the first release closure has none.
        host_dependencies: Vec<OvenRuntimeFoundationProviderHostDependency>,
    },
}

/// One already-selected host dependency used to compile a declared build script.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OvenRuntimeFoundationProviderHostDependency {
    /// Rust-facing alias supplied by the original source selection.
    pub(crate) alias: String,
    /// Selected host-unit identity; this is a reference, not a new dependency edge resolver.
    pub(crate) unit: String,
}

/// Exact response schema emitted by the Incan-authored provider-intake exchange.
pub(crate) const OVEN_RUNTIME_FOUNDATION_PROVIDER_INTAKE_REQUEST_SCHEMA: &str =
    "incan.oven.runtime-foundation-provider-intake-request/2";
const OVEN_RUNTIME_FOUNDATION_PROVIDER_INTAKE_SCHEMA: &str = "incan.oven.runtime-foundation-provider-intake/2";

/// One original manifest build-dependency slot retained only for the host-to-source intake request.
///
/// The source bridge checks that `declaration_index` names a literal `build-dependencies` declaration before it
/// reduces this record to the alias/unit pair returned in a provider declaration. Keeping the slot in request evidence
/// prevents a host from reconstructing that source correlation from a selected graph, which deliberately contains no
/// manifest parser or dependency-role projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OvenRuntimeFoundationProviderIntakeHostDependency {
    /// Stable ordinal of the original selected manifest dependency declaration.
    pub(crate) declaration_index: u32,
    /// Rust-facing alias from that original declaration.
    pub(crate) alias: String,
    /// Existing selected host-domain unit identity bound by the source producer.
    pub(crate) unit: String,
}

/// Exact source request encoded only from host-validated evidence.
#[derive(Serialize)]
struct OvenRuntimeFoundationProviderIntakeWireRequest<'a> {
    schema: &'static str,
    request: OvenRuntimeFoundationProviderIntakeWireRequestBody<'a>,
}

/// Closed request body shared only with the Incan provider-intake source contract.
#[derive(Serialize)]
struct OvenRuntimeFoundationProviderIntakeWireRequestBody<'a> {
    selected_identity: &'a str,
    manifest: &'a OvenSelectedRustFacetSourceMember,
    manifest_source: &'a str,
    package_members: &'a [OvenSelectedRustFacetSourceMember],
    host_dependencies: &'a [OvenRuntimeFoundationProviderIntakeHostDependency],
}

/// Closed wire response accepted only as a source declaration candidate. It has no execution authority.
#[derive(Debug, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
enum OvenRuntimeFoundationProviderIntakeWireResponse {
    /// The checked source declaration selected no build script.
    NoBuildScript { schema: String, selected_identity: String },
    /// The checked source declaration selected one build script and its explicitly supplied package inventory.
    BuildScript {
        schema: String,
        selected_identity: String,
        declaration: OvenRuntimeFoundationProviderIntakeWireDeclaration,
    },
    /// Source parsing or declaration binding refused; this cannot become a no-provider outcome by omission.
    Refused {
        schema: String,
        selected_identity: String,
        error: OvenRuntimeFoundationProviderIntakeWireRefusal,
    },
}

/// Source-only declaration facts which still require selected-graph authentication by the Rust host.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OvenRuntimeFoundationProviderIntakeWireDeclaration {
    entrypoint: OvenRuntimeFoundationProviderIntakeWireSourceMember,
    package_members: Vec<OvenRuntimeFoundationProviderIntakeWireSourceMember>,
    host_dependencies: Vec<OvenRuntimeFoundationProviderIntakeWireHostDependency>,
}

/// One portable package member as represented by the Incan source exchange.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OvenRuntimeFoundationProviderIntakeWireSourceMember {
    path: String,
    digest: String,
}

/// One source-validated build-dependency slot reduced to the selected alias/unit edge.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OvenRuntimeFoundationProviderIntakeWireHostDependency {
    alias: String,
    unit: String,
}

/// Host-retained evidence for one exact provider-intake Engine request.
///
/// The Engine command path must construct this only after it has rehashed the retained package manifest and complete
/// package inventory, bound declaration slots to selected host units, and issued the exact request permit. The Incan
/// response carries no roots or manifest bytes; this record restores those facts without a second graph, filesystem
/// probe or Cargo metadata query.
#[derive(Debug, Clone)]
pub(crate) struct OvenRuntimeFoundationProviderIntakeEvidence {
    /// Existing selected Rust unit which this source declaration may describe.
    selected_identity: String,
    /// Complete physical package evidence retained by the host request.
    package: OvenRuntimeFoundationProviderPackageSource,
    /// Source-declared build-dependency slots already correlated to selected host units by the producer.
    host_dependencies: Vec<OvenRuntimeFoundationProviderIntakeHostDependency>,
}

/// Closed refusal details are decoded only to keep malformed source output from becoming an implicit no-provider state.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OvenRuntimeFoundationProviderIntakeWireRefusal {
    kind: String,
    fields: Vec<String>,
}

/// Build-time provider evidence retained by a runtime-foundation asset.
///
/// This is not a second Rust dependency graph. The selected graph remains the compilation authority; captured output
/// facts below must agree with its generated-input, cfg and environment projections.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum OvenRuntimeFoundationProviderState {
    /// The publisher checked the explicit `NoBuildScript` declaration and found no provider effect.
    NoProvider,
    /// The publisher captured one canonical typed provider receipt for the declared build script.
    Captured {
        /// Complete canonical provider receipt whose identities are derived from its declared effects.
        receipt: OvenRuntimeFoundationProviderReceipt,
    },
    /// The publisher observed a provider requirement it cannot yet model without weakening the foundation contract.
    Unsupported {
        /// Stable human-readable reason rendered in the typed unsupported diagnostic.
        reason: String,
    },
}

/// Schema version for a typed build-script receipt embedded in a foundation asset.
pub(crate) const OVEN_RUNTIME_FOUNDATION_PROVIDER_RECEIPT_SCHEMA_VERSION: u32 = 1;

/// Canonical receipt for one completed declared build-script execution.
///
/// Both identities are recomputed during admission. They are not opaque producer assertions: `effect_digest` binds
/// the complete normalized effects, and `provider_receipt_identity` additionally binds the selected unit and its
/// declared script input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OvenRuntimeFoundationProviderReceipt {
    /// Receipt wire schema checked before its effect payload is interpreted.
    pub(crate) schema_version: u32,
    /// Canonical identity of the selected unit, declaration and complete normalized effects.
    pub(crate) provider_receipt_identity: String,
    /// Canonical digest of only the complete normalized effect record.
    pub(crate) effect_digest: String,
    /// Complete effect record emitted by this one execution.
    pub(crate) effects: OvenRuntimeFoundationProviderEffects,
}

/// Complete normalized effects of one build-script execution.
///
/// The selected graph consumes generated inputs, cfg and environment from this record. Check-cfg and rerun facts
/// remain here too, so they cannot disappear behind an opaque digest before the direct-Rustc command learns how to
/// consume them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OvenRuntimeFoundationProviderEffects {
    /// Exact generated inputs this provider contributed to the selected unit.
    pub(crate) generated_inputs: Vec<OvenSelectedRustFacetGeneratedInput>,
    /// Provider-emitted cfg values retained by the selected unit.
    pub(crate) emitted_cfg: Vec<String>,
    /// `rustc-check-cfg` directives retained until they have a typed direct-Rustc command projection.
    pub(crate) checked_cfg: Vec<String>,
    /// Provider-emitted environment values retained by the selected unit.
    pub(crate) emitted_environment: BTreeMap<String, OvenSelectedRustFacetEnvironmentValue>,
    /// Source-member-relative rerun observations emitted by the provider.
    pub(crate) rerun_paths: Vec<String>,
    /// Environment names whose values the provider asked its producer to observe for rerun.
    pub(crate) rerun_environment: Vec<String>,
    /// Explicit native-link outcome from this provider execution.
    ///
    /// The first source-backed foundation can admit only an observed empty outcome. A provider which emitted link
    /// directives remains representable as release provenance, but source rebuild refuses it until the direct-Rustc
    /// link-plan schema can carry those directives without opaque passthrough.
    pub(crate) native_link: OvenRuntimeFoundationNativeLinkState,
}

/// Native-link outcome retained with a captured provider execution.
///
/// This is deliberately a distinct state from an absent provider record. `NoNativeLink` means that the provider
/// ran and its normalized effect record contained no native-link directive. It prevents a missing field from being
/// silently interpreted as an empty link plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum OvenRuntimeFoundationNativeLinkState {
    /// The provider emitted no native library, framework, search-path or linker-argument directive.
    NoNativeLink,
    /// The provider emitted a native-link requirement which this schema cannot yet express safely.
    Unsupported {
        /// Stable human-readable reason rendered in the typed unsupported diagnostic.
        reason: String,
    },
}

/// Bounded stdout retained only while a provider runner turns build-script directives into a typed receipt.
///
/// The value is deliberately not serialized or logged. In particular, `rustc-env` values can be sensitive; the
/// later selected-graph binding converts a matching value into its existing text/path/digest representation before
/// any foundation descriptor is sealed.
const OVEN_RUNTIME_FOUNDATION_PROVIDER_STDOUT_LIMIT: usize = 1024 * 1024;

/// Parsed build-script directives before they are bound to selected graph facts.
///
/// This is not a second provider graph or an authority record. It is an ephemeral, non-serializable parser result
/// whose raw environment values must be compared with the selected graph before an effect record is constructed.
#[derive(PartialEq, Eq)]
struct OvenRuntimeFoundationProviderDirectives {
    emitted_cfg: Vec<String>,
    checked_cfg: Vec<String>,
    emitted_environment: BTreeMap<String, String>,
    rerun_paths: Vec<String>,
    rerun_environment: Vec<String>,
    native_link: OvenRuntimeFoundationNativeLinkState,
}

/// Execution policy for one selected Rust unit within a sealed runtime foundation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OvenRuntimeFoundationUnit {
    /// Selected-source identity naming exactly one unit in the foundation selected graph.
    pub(crate) selected_identity: String,
    /// Explicit host or target compilation domain; callers must never infer this from an artifact filename.
    pub(crate) domain: OvenSelectedRustFacetDomain,
    /// Whether the foundation supplies an immutable artifact or Oven may rebuild compiler-owned source.
    pub(crate) execution: OvenRuntimeFoundationUnitExecution,
}

/// The source-to-artifact policy for one selected runtime unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub(crate) enum OvenRuntimeFoundationUnitExecution {
    /// A sealed third-party artifact is already present in the SDK foundation.
    Prebuilt {
        /// Exact artifact that direct Rustc may expose through a selected dependency edge.
        artifact: OvenRustcArtifactExtern,
    },
    /// A compiler-owned source unit must be compiled by Oven above the sealed third-party foundation.
    Rebuild,
}

/// One direct prebuilt dependency projected from a selected graph edge.
///
/// The dependency alias comes from the shared selected graph rather than an artifact filename. This preserves package
/// renames and makes the projection a physical compiler-input adapter instead of a second dependency resolver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OvenRuntimeFoundationPrebuiltDependency {
    /// Rust-facing dependency alias passed to rustc --extern.
    pub(crate) alias: String,
    /// Selected identity of the prebuilt child unit.
    pub(crate) selected_identity: String,
    /// Explicit host or target domain of the child artifact.
    pub(crate) domain: OvenSelectedRustFacetDomain,
    /// Digest-verified SDK artifact for the child unit.
    pub(crate) artifact: OvenRustcArtifactExtern,
}

/// One direct prebuilt dependency rebound to its verified foundation-owned physical artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OvenMaterializedRuntimeFoundationPrebuiltDependency {
    /// Rust-facing dependency alias passed to direct Rustc.
    pub(crate) alias: String,
    /// Selected identity of the prebuilt child unit.
    pub(crate) selected_identity: String,
    /// Explicit host or target domain of the prebuilt child unit.
    pub(crate) domain: OvenSelectedRustFacetDomain,
    /// Foundation-owned immutable artifact path verified during publication materialization.
    pub(crate) artifact: PathBuf,
    /// Digest recorded for the exact artifact path.
    pub(crate) digest: String,
}

/// One provider package source closure rebound through an admitted runtime-foundation asset.
///
/// The source paths exist only because the owning release asset first verified their declared byte identities. A
/// later provider executor must receive this carrier together with its matching provider record; it must not rebuild
/// a build-script path from a manifest, cache or caller-selected checkout.
#[derive(Debug, Clone)]
pub(crate) struct OvenMaterializedRuntimeFoundationProviderBuildScript {
    source_root: PathBuf,
    entrypoint: PathBuf,
    source_members: BTreeMap<String, PathBuf>,
}

impl OvenMaterializedRuntimeFoundationProviderBuildScript {
    /// Return the verified physical source root containing the complete declared provider source closure.
    pub(crate) fn source_root(&self) -> &Path {
        &self.source_root
    }

    /// Return the verified build-script entrypoint named by the admitted declaration.
    pub(crate) fn entrypoint(&self) -> &Path {
        &self.entrypoint
    }

    /// Resolve one declared package source member after asset admission.
    pub(crate) fn source_member(&self, path: &str) -> Option<&Path> {
        self.source_members.get(path).map(PathBuf::as_path)
    }
}

/// Publisher-only physical projection of a sealed runtime foundation.
///
/// The caller supplies roots held by existing Store or Loaf leases. This adapter verifies the complete selected source
/// trees and sealed artifact catalogue once, then hands later direct-Rustc code only admitted physical files and
/// topological rebuild order. It neither locates a cache nor reads Cargo metadata.
#[derive(Debug, Clone)]
pub(crate) struct OvenMaterializedRuntimeFoundation {
    sources: OvenMaterializedRustFacetGraph,
    artifact_plan: OvenRustcArtifactPlan,
    rebuild_order: Vec<String>,
    prebuilt_dependencies: BTreeMap<String, Vec<OvenMaterializedRuntimeFoundationPrebuiltDependency>>,
}

/// Publisher-only physical projection paired with the admitted release asset that authorized it.
///
/// A bare [`OvenMaterializedRuntimeFoundation`] is useful for lower-level materialization tests, but it does not
/// prove that an exhaustive provider catalogue was admitted. Downstream runtime-closure publication must consume this
/// wrapper so its provenance begins at the validated release asset while its reuse identity can remain limited to
/// effective compiler inputs.
#[derive(Debug, Clone)]
pub(crate) struct OvenMaterializedRuntimeFoundationAsset {
    foundation_identity: String,
    foundation: ValidatedOvenRuntimeFoundation,
    providers: BTreeMap<String, OvenRuntimeFoundationProviderRecord>,
    provider_build_scripts: BTreeMap<String, OvenMaterializedRuntimeFoundationProviderBuildScript>,
    materialized: OvenMaterializedRuntimeFoundation,
}

/// An admitted runtime foundation that retains canonical selected source facts and execution policy.
#[derive(Debug, Clone)]
pub(crate) struct ValidatedOvenRuntimeFoundation {
    compiler_closure_digest: String,
    artifact_owner: String,
    artifacts: OvenRustcArtifactManifest,
    selected_graph: ValidatedOvenSelectedRustFacetGraph,
    units: BTreeMap<String, OvenRuntimeFoundationUnit>,
}

/// An admitted release foundation asset with complete provider-state classification.
#[derive(Debug, Clone)]
pub(crate) struct ValidatedOvenRuntimeFoundationAsset {
    foundation_identity: String,
    foundation: ValidatedOvenRuntimeFoundation,
    providers: BTreeMap<String, OvenRuntimeFoundationProviderRecord>,
}

/// A release asset whose descriptor and complete foundation-owned file set have passed publisher-time admission.
///
/// The toolchain root remains separate because it is a named Toolchain owner, not content hidden underneath an SDK
/// foundation. This type does not lease or select anything for normal execution; it exists only for explicit bake or
/// release publication work.
#[derive(Debug, Clone)]
pub(crate) struct OvenAdmittedRuntimeFoundationAsset {
    asset: ValidatedOvenRuntimeFoundationAsset,
    owner_roots: Vec<OvenSelectedRustFacetOwnerRoot>,
}

impl OvenRuntimeFoundation {
    /// Canonicalize and validate an SDK runtime foundation without consulting Cargo, the ambient filesystem, or a
    /// neighbouring build cache.
    ///
    /// The result proves that every selected unit has exactly one declared execution policy, all prebuilt units are
    /// registry-backed immutable artifacts present in the sealed manifest, and every rebuildable unit is an admitted
    /// compiler-owned library or procedural macro.
    pub(crate) fn validated(self) -> Result<ValidatedOvenRuntimeFoundation, OvenRustcError> {
        let Self {
            schema_version,
            compiler_closure_digest,
            artifact_owner,
            artifacts,
            selected_graph,
            units,
        } = self;
        if schema_version != OVEN_RUNTIME_FOUNDATION_SCHEMA_VERSION {
            return Err(runtime_foundation_invalid(
                "runtime foundation schema",
                format!("expected schema {OVEN_RUNTIME_FOUNDATION_SCHEMA_VERSION}, found {schema_version}"),
            ));
        }
        validate_sha256_identity(&compiler_closure_digest, "runtime foundation compiler closure")?;
        validate_sha256_identity(&artifact_owner, "runtime foundation artifact owner")?;
        let selected_graph = selected_graph
            .validated()
            .map_err(|error| runtime_foundation_invalid("runtime foundation selected graph", error.to_string()))?;
        let artifact_owner_kind = selected_graph
            .graph()
            .owners
            .iter()
            .find(|owner| owner.identity == artifact_owner)
            .map(|owner| owner.kind)
            .ok_or_else(|| {
                runtime_foundation_invalid(
                    "runtime foundation artifact owner",
                    "is absent from the selected graph owner table",
                )
            })?;
        if artifact_owner_kind != OvenSelectedRustFacetOwnerKind::Constituent {
            return Err(runtime_foundation_invalid(
                "runtime foundation artifact owner",
                "must name the sealed constituent that owns the foundation root",
            ));
        }
        let expected_intent = &selected_graph.graph().selection.intent;
        if artifacts.intent.target != expected_intent.target
            || artifacts.intent.toolchain != expected_intent.toolchain
            || artifacts.intent.profile != expected_intent.profile
            || artifacts.intent.features != expected_intent.features
        {
            return Err(runtime_foundation_invalid(
                "runtime foundation artifact intent",
                "does not match the selected graph intent",
            ));
        }
        artifacts.validate_shape(&artifacts.intent)?;
        let declared_artifacts = artifacts.declared_artifact_digests()?;
        let graph_units = selected_graph
            .graph()
            .units
            .iter()
            .map(|unit| (unit.identity.as_str(), unit))
            .collect::<BTreeMap<_, _>>();
        let mut policies = BTreeMap::new();
        for policy in units {
            validate_sha256_identity(&policy.selected_identity, "runtime foundation unit identity")?;
            let Some(unit) = graph_units.get(policy.selected_identity.as_str()) else {
                return Err(runtime_foundation_invalid(
                    "runtime foundation unit identity",
                    format!("does not name a selected graph unit {}", policy.selected_identity),
                ));
            };
            let unit = *unit;
            if policy.domain != unit.domain {
                return Err(runtime_foundation_invalid(
                    "runtime foundation unit domain",
                    format!(
                        "unit {} declares {:?} but its selected graph unit is {:?}",
                        unit.crate_name, policy.domain, unit.domain
                    ),
                ));
            }
            validate_runtime_unit_policy(unit, &policy, &artifact_owner, &artifacts, &declared_artifacts)?;
            if policies.insert(policy.selected_identity.clone(), policy).is_some() {
                return Err(runtime_foundation_invalid(
                    "runtime foundation units",
                    "declares one selected unit more than once",
                ));
            }
        }
        let declared = policies.keys().map(String::as_str).collect::<BTreeSet<_>>();
        let selected = graph_units.keys().copied().collect::<BTreeSet<_>>();
        if declared != selected {
            let missing = selected.difference(&declared).copied().collect::<Vec<_>>();
            let extra = declared.difference(&selected).copied().collect::<Vec<_>>();
            let mut message = Vec::new();
            if !missing.is_empty() {
                message.push(format!("omits selected unit(s): {}", missing.join(", ")));
            }
            if !extra.is_empty() {
                message.push(format!("declares unknown unit(s): {}", extra.join(", ")));
            }
            return Err(runtime_foundation_invalid(
                "runtime foundation units",
                message.join("; "),
            ));
        }
        Ok(ValidatedOvenRuntimeFoundation {
            compiler_closure_digest,
            artifact_owner,
            artifacts,
            selected_graph,
            units: policies,
        })
    }
}

impl OvenRuntimeFoundationAsset {
    /// Construct a canonical runtime-foundation asset after checking all selected-unit provider states.
    pub(crate) fn sealed(
        mut foundation: OvenRuntimeFoundation,
        mut providers: Vec<OvenRuntimeFoundationProviderRecord>,
    ) -> Result<Self, OvenRustcError> {
        canonicalize_runtime_foundation_asset_facts(&mut foundation, &mut providers)?;
        seal_runtime_foundation_provider_receipts(&mut providers)?;
        let foundation_identity = runtime_foundation_asset_identity(&foundation, &providers)?;
        let asset = Self {
            schema_version: OVEN_RUNTIME_FOUNDATION_ASSET_SCHEMA_VERSION,
            foundation_identity,
            foundation,
            providers,
        };
        let _ = asset.clone().validated()?;
        Ok(asset)
    }

    /// Canonicalize and validate this complete release asset without locating any physical root.
    pub(crate) fn validated(self) -> Result<ValidatedOvenRuntimeFoundationAsset, OvenRustcError> {
        let Self {
            schema_version,
            foundation_identity,
            mut foundation,
            mut providers,
        } = self;
        if schema_version != OVEN_RUNTIME_FOUNDATION_ASSET_SCHEMA_VERSION {
            return Err(runtime_foundation_invalid(
                "runtime foundation asset schema",
                format!("expected schema {OVEN_RUNTIME_FOUNDATION_ASSET_SCHEMA_VERSION}, found {schema_version}"),
            ));
        }
        canonicalize_runtime_foundation_asset_facts(&mut foundation, &mut providers)?;
        validate_sha256_identity(&foundation_identity, "runtime foundation asset identity")?;
        let expected_identity = runtime_foundation_asset_identity(&foundation, &providers)?;
        if foundation_identity != expected_identity {
            return Err(runtime_foundation_invalid(
                "runtime foundation asset identity",
                "does not match its canonical foundation and provider-output facts",
            ));
        }
        let foundation = foundation.validated()?;
        let providers = validate_runtime_foundation_provider_records(&foundation, providers)?;
        Ok(ValidatedOvenRuntimeFoundationAsset {
            foundation_identity,
            foundation,
            providers,
        })
    }
}

impl ValidatedOvenRuntimeFoundation {
    /// Return the direct compiler closure identity that participates in every compiled-unit identity.
    pub(crate) fn compiler_closure_digest(&self) -> &str {
        &self.compiler_closure_digest
    }

    /// Return the sole selected owner whose root carries this foundation's source and artifact payload.
    pub(crate) fn artifact_owner(&self) -> &str {
        &self.artifact_owner
    }

    /// Borrow the sealed artifact catalogue while its validated foundation remains owned by the caller.
    pub(crate) fn artifacts(&self) -> &OvenRustcArtifactManifest {
        &self.artifacts
    }

    /// Borrow the sole selected source/unit graph used by direct Rustc and inspection.
    pub(crate) fn selected_graph(&self) -> &ValidatedOvenSelectedRustFacetGraph {
        &self.selected_graph
    }

    /// Return the declared source-to-artifact policy for one selected unit.
    pub(crate) fn unit_policy(&self, selected_identity: &str) -> Option<&OvenRuntimeFoundationUnit> {
        self.units.get(selected_identity)
    }

    /// Iterate the compiler-owned units that the publisher must compile above the immutable third-party foundation.
    pub(crate) fn rebuild_units(&self) -> impl Iterator<Item = &OvenRuntimeFoundationUnit> {
        self.units
            .values()
            .filter(|unit| matches!(unit.execution, OvenRuntimeFoundationUnitExecution::Rebuild))
    }

    /// Project only prebuilt direct dependencies of one selected parent unit.
    ///
    /// Rebuildable children are deliberately absent: the direct-Rustc publisher supplies their caller-owned outputs
    /// after it compiles their own selected units. This derives aliases and edges from the shared selected graph,
    /// preventing an artifact catalogue from becoming a second dependency graph.
    pub(crate) fn prebuilt_dependencies(
        &self,
        selected_identity: &str,
    ) -> Result<Vec<OvenRuntimeFoundationPrebuiltDependency>, OvenRustcError> {
        let unit = self
            .selected_graph
            .graph()
            .units
            .iter()
            .find(|unit| unit.identity == selected_identity)
            .ok_or_else(|| {
                runtime_foundation_invalid(
                    "runtime foundation dependency parent",
                    format!("does not name a selected unit {selected_identity}"),
                )
            })?;
        let mut dependencies = Vec::new();
        for dependency in &unit.dependencies {
            let policy = self.units.get(&dependency.unit).ok_or_else(|| {
                runtime_foundation_invalid(
                    "runtime foundation dependency",
                    format!("selected child {} has no execution policy", dependency.unit),
                )
            })?;
            if let OvenRuntimeFoundationUnitExecution::Prebuilt { artifact } = &policy.execution {
                dependencies.push(OvenRuntimeFoundationPrebuiltDependency {
                    alias: dependency.alias.clone(),
                    selected_identity: dependency.unit.clone(),
                    domain: policy.domain,
                    artifact: artifact.clone(),
                });
            }
        }
        Ok(dependencies)
    }

    /// Verify and bind this foundation's selected source trees and artifact catalogue for one publisher invocation.
    ///
    /// This is deliberately publisher-only work. A normal command receives an already selected and leased plan; it
    /// must not invoke this method as a way to probe an SDK root or construct a new direct-Rustc closure.
    pub(crate) fn materialize_for_publication(
        &self,
        owner_roots: &[OvenSelectedRustFacetOwnerRoot],
    ) -> Result<OvenMaterializedRuntimeFoundation, OvenRustcError> {
        self.materialize_for_publication_with_supplemental_source_members(owner_roots, &[])
    }

    /// Verify and bind this foundation while retaining an already-admitted provider-only source closure.
    ///
    /// The generic selected graph still supplies every compiler-visible input and compiled-unit identity. The
    /// supplemental closure merely makes its package-local physical tree exact before the provider record is handed
    /// to a later build-script executor.
    fn materialize_for_publication_with_supplemental_source_members(
        &self,
        owner_roots: &[OvenSelectedRustFacetOwnerRoot],
        supplemental_source_members: &[OvenSelectedRustFacetSupplementalSourceMembers],
    ) -> Result<OvenMaterializedRuntimeFoundation, OvenRustcError> {
        let artifact_root = owner_roots
            .iter()
            .find(|owner| owner.identity == self.artifact_owner)
            .map(|owner| owner.root.clone())
            .ok_or_else(|| {
                runtime_foundation_invalid(
                    "runtime foundation artifact owner",
                    "has no physical root for publication",
                )
            })?;
        let sources = materialize_selected_rust_facet_graph_with_supplemental_source_members(
            &self.selected_graph,
            &self.compiler_closure_digest,
            owner_roots,
            supplemental_source_members,
        )?;
        let _ = self
            .artifacts
            .materialized_artifacts(&artifact_root, &self.artifacts.intent)?;
        let artifact_plan = self
            .artifacts
            .materialize_trusted_store(&artifact_root, &self.artifacts.intent)?;
        let paths = artifact_plan
            .externs
            .iter()
            .map(|(crate_name, path)| (crate_name.as_str(), path.clone()))
            .collect::<BTreeMap<_, _>>();
        let rebuild_order = runtime_foundation_rebuild_order(self.selected_graph.graph(), &self.units)?;
        let mut prebuilt_dependencies = BTreeMap::new();
        for selected_identity in &rebuild_order {
            let dependencies = self
                .prebuilt_dependencies(selected_identity)?
                .into_iter()
                .map(|dependency| {
                    let artifact = paths
                        .get(dependency.artifact.crate_name.as_str())
                        .cloned()
                        .ok_or_else(|| {
                            runtime_foundation_invalid(
                                "runtime foundation prebuilt artifact",
                                format!(
                                    "unit {} lost materialized direct extern {}",
                                    dependency.selected_identity, dependency.artifact.crate_name
                                ),
                            )
                        })?;
                    Ok(OvenMaterializedRuntimeFoundationPrebuiltDependency {
                        alias: dependency.alias,
                        selected_identity: dependency.selected_identity,
                        domain: dependency.domain,
                        artifact,
                        digest: dependency.artifact.digest,
                    })
                })
                .collect::<Result<Vec<_>, OvenRustcError>>()?;
            prebuilt_dependencies.insert(selected_identity.clone(), dependencies);
        }
        Ok(OvenMaterializedRuntimeFoundation {
            sources,
            artifact_plan,
            rebuild_order,
            prebuilt_dependencies,
        })
    }
}

impl OvenMaterializedRuntimeFoundation {
    /// Borrow the full-source-verified selected unit projection used by the publisher.
    pub(crate) fn sources(&self) -> &OvenMaterializedRustFacetGraph {
        &self.sources
    }

    /// Borrow the complete sealed artifact plan after its publisher-time byte verification.
    pub(crate) fn artifact_plan(&self) -> &OvenRustcArtifactPlan {
        &self.artifact_plan
    }

    /// Iterate selected identities in the dependency-safe order direct Rustc must rebuild them.
    pub(crate) fn rebuild_order(&self) -> impl Iterator<Item = &str> {
        self.rebuild_order.iter().map(String::as_str)
    }

    /// Borrow direct prebuilt dependencies for one compiler-owned unit.
    pub(crate) fn prebuilt_dependencies(
        &self,
        selected_identity: &str,
    ) -> Option<&[OvenMaterializedRuntimeFoundationPrebuiltDependency]> {
        self.prebuilt_dependencies.get(selected_identity).map(Vec::as_slice)
    }
}

impl OvenMaterializedRuntimeFoundationAsset {
    /// Return the canonical release-asset identity that admitted this physical projection.
    pub(crate) fn foundation_identity(&self) -> &str {
        &self.foundation_identity
    }

    /// Borrow the selected source/artifact policy admitted from the same release asset.
    pub(crate) fn foundation(&self) -> &ValidatedOvenRuntimeFoundation {
        &self.foundation
    }

    /// Return the admitted declaration and provider receipt for one selected unit.
    ///
    /// This is the only hand-off the later direct-Rustc build-script runner may use. It keeps execution bound to the
    /// asset whose provider state was checked before physical source materialization, rather than accepting a loose
    /// declaration beside a caller-selected source path.
    pub(crate) fn provider_record(&self, selected_identity: &str) -> Option<&OvenRuntimeFoundationProviderRecord> {
        self.providers.get(selected_identity)
    }

    /// Return the provider-only physical source closure for one admitted build script.
    ///
    /// A caller must pair this with [`Self::provider_record`] for the same selected identity. This prevents a later
    /// executor from accepting a standalone path or re-discovering the script from a manifest/cache layout.
    pub(crate) fn provider_build_script(
        &self,
        selected_identity: &str,
    ) -> Option<&OvenMaterializedRuntimeFoundationProviderBuildScript> {
        self.provider_build_scripts.get(selected_identity)
    }

    /// Borrow the physical source/artifact paths verified through this release asset.
    pub(crate) fn materialized(&self) -> &OvenMaterializedRuntimeFoundation {
        &self.materialized
    }
}

impl ValidatedOvenRuntimeFoundationAsset {
    /// Return the canonical content identity of this release-owned foundation authority.
    pub(crate) fn foundation_identity(&self) -> &str {
        &self.foundation_identity
    }

    /// Borrow the selected source/artifact foundation admitted by this release asset.
    pub(crate) fn foundation(&self) -> &ValidatedOvenRuntimeFoundation {
        &self.foundation
    }

    /// Return the exhaustive build-time provider state for one selected unit.
    pub(crate) fn provider_state(&self, selected_identity: &str) -> Option<&OvenRuntimeFoundationProviderState> {
        self.providers.get(selected_identity).map(|record| &record.state)
    }

    /// Return the selected build-script declaration and its exhaustive provider state for one selected unit.
    ///
    /// The direct-Rustc runner needs the declaration later to compile exactly the producer-selected entrypoint and
    /// host closure. Keeping it with the admitted asset prevents an execution layer from reconstructing build-script
    /// ownership from a manifest or cache layout.
    pub(crate) fn provider_record(&self, selected_identity: &str) -> Option<&OvenRuntimeFoundationProviderRecord> {
        self.providers.get(selected_identity)
    }

    /// Bind this asset to physical publisher roots only when no unit has an unsupported provider requirement.
    ///
    /// An unsupported record remains useful release provenance, but it cannot be silently treated as an empty
    /// generated-output catalogue while materializing a source-backed foundation.
    pub(crate) fn materialize_for_publication(
        &self,
        owner_roots: &[OvenSelectedRustFacetOwnerRoot],
    ) -> Result<OvenMaterializedRuntimeFoundation, OvenRustcError> {
        let unsupported = self
            .providers
            .iter()
            .find_map(|(selected_identity, record)| match &record.state {
                OvenRuntimeFoundationProviderState::Unsupported { reason } => {
                    Some((selected_identity.as_str(), "provider facts", reason.as_str()))
                }
                OvenRuntimeFoundationProviderState::Captured {
                    receipt:
                        OvenRuntimeFoundationProviderReceipt {
                            effects:
                                OvenRuntimeFoundationProviderEffects {
                                    native_link: OvenRuntimeFoundationNativeLinkState::Unsupported { reason },
                                    ..
                                },
                            ..
                        },
                    ..
                } => Some((selected_identity.as_str(), "native-link facts", reason.as_str())),
                OvenRuntimeFoundationProviderState::NoProvider
                | OvenRuntimeFoundationProviderState::Captured { .. } => None,
            });
        if let Some((selected_identity, fact_kind, reason)) = unsupported {
            return Err(runtime_foundation_invalid(
                "runtime foundation providers",
                format!("selected unit {selected_identity} has unsupported {fact_kind}: {reason}"),
            ));
        }
        let mut supplemental_source_members = Vec::new();
        for record in self.providers.values() {
            let package = match &record.declaration {
                OvenRuntimeFoundationProviderDeclaration::NoBuildScript { package }
                | OvenRuntimeFoundationProviderDeclaration::BuildScript { package, .. } => package,
            };
            let mut members = Vec::with_capacity(package.members.len() + 1);
            members.push(package.manifest.clone());
            members.extend(package.members.clone());
            members.sort();
            supplemental_source_members.push(OvenSelectedRustFacetSupplementalSourceMembers {
                selected_identity: record.selected_identity.clone(),
                source_root: package.root.clone(),
                members,
            });
        }
        self.foundation
            .materialize_for_publication_with_supplemental_source_members(owner_roots, &supplemental_source_members)
    }
}

impl OvenAdmittedRuntimeFoundationAsset {
    /// Return the canonical release-asset identity admitted from the foundation descriptor.
    pub(crate) fn foundation_identity(&self) -> &str {
        self.asset.foundation_identity()
    }

    /// Bind the admitted physical roots to direct-Rustc publication after all descriptor and member checks passed.
    pub(crate) fn materialize_for_publication(&self) -> Result<OvenMaterializedRuntimeFoundation, OvenRustcError> {
        self.asset.materialize_for_publication(&self.owner_roots)
    }

    /// Materialize the release asset for downstream runtime publication while retaining its provider-gated identity.
    ///
    /// This is the hand-off for direct-Rustc execution and runtime-closure publication. It cannot be constructed from
    /// a bare selected graph or a loose foundation descriptor, so a later closure records the asset whose exhaustive
    /// provider state was actually admitted.
    pub(crate) fn materialize_asset_for_publication(
        &self,
    ) -> Result<OvenMaterializedRuntimeFoundationAsset, OvenRustcError> {
        let materialized = self.asset.materialize_for_publication(&self.owner_roots)?;
        let provider_build_scripts =
            materialize_runtime_foundation_provider_build_scripts(&self.asset.providers, &materialized)?;
        Ok(OvenMaterializedRuntimeFoundationAsset {
            foundation_identity: self.asset.foundation_identity().to_string(),
            foundation: self.asset.foundation().clone(),
            providers: self.asset.providers.clone(),
            provider_build_scripts,
            materialized,
        })
    }
}

/// Rebind every admitted provider source closure through its already verified package root.
///
/// This intentionally happens after generic selected-source materialization accepted the complete union of
/// compiler-visible and provider-only members. The second per-file digest read closes the hand-off to the retained
/// runtime asset: a later executor receives only paths that still match the declaration carried by that asset.
fn materialize_runtime_foundation_provider_build_scripts(
    providers: &BTreeMap<String, OvenRuntimeFoundationProviderRecord>,
    materialized: &OvenMaterializedRuntimeFoundation,
) -> Result<BTreeMap<String, OvenMaterializedRuntimeFoundationProviderBuildScript>, OvenRustcError> {
    let mut build_scripts = BTreeMap::new();
    for (selected_identity, record) in providers {
        let OvenRuntimeFoundationProviderDeclaration::BuildScript {
            package, entrypoint, ..
        } = &record.declaration
        else {
            continue;
        };
        let source_root = materialized
            .sources()
            .supplemental_source_root(&package.root)
            .ok_or_else(|| {
                runtime_foundation_invalid(
                    "runtime foundation provider source",
                    format!("selected unit {selected_identity} has no materialized package root"),
                )
            })?;
        let mut source_members = BTreeMap::new();
        for member in std::iter::once(&package.manifest).chain(&package.members) {
            let path = safe_path(source_root, &member.path, "runtime foundation provider source member")?;
            let path = verified_regular_file(&path, "runtime foundation provider source member")?;
            let bytes = fs::read(&path).map_err(|source| OvenRustcError::Io {
                path: path.clone(),
                source,
            })?;
            let actual = digest_bytes(&bytes);
            if actual != member.digest {
                return Err(OvenRustcError::ArtifactDigestMismatch {
                    path,
                    expected: member.digest.clone(),
                    actual,
                });
            }
            if source_members.insert(member.path.clone(), path).is_some() {
                return Err(runtime_foundation_invalid(
                    "runtime foundation provider source",
                    format!(
                        "selected unit {selected_identity} repeats package source member `{}`",
                        member.path
                    ),
                ));
            }
        }
        let entrypoint = source_members.get(entrypoint).cloned().ok_or_else(|| {
            runtime_foundation_invalid(
                "runtime foundation provider source",
                format!("selected unit {selected_identity} has no materialized build-script entrypoint"),
            )
        })?;
        if build_scripts
            .insert(
                selected_identity.clone(),
                OvenMaterializedRuntimeFoundationProviderBuildScript {
                    source_root: source_root.to_path_buf(),
                    entrypoint,
                    source_members,
                },
            )
            .is_some()
        {
            return Err(runtime_foundation_invalid(
                "runtime foundation provider source",
                format!("selected unit {selected_identity} repeats a build-script declaration"),
            ));
        }
    }
    Ok(build_scripts)
}

/// Publish one complete provider-supplied runtime foundation into an installed asset root.
///
/// The caller must supply the selected foundation and exhaustive provider facts explicitly; this function never
/// reads Cargo metadata, searches a target directory, or reconstructs a provider result. It first materializes the
/// supplied roots to verify every source/artifact byte and provider gate, copies only descriptor-derived foundation
/// members into a sibling staging directory, re-admits that staged payload, and only then atomically exposes it at
/// `destination`.
///
/// The separately held Toolchain owner is verified but never copied into this asset root. It stays part of the
/// installed compiler distribution rather than becoming an unrecorded foundation member.
pub(crate) fn publish_runtime_foundation_asset(
    asset: OvenRuntimeFoundationAsset,
    source_foundation_root: &Path,
    toolchain_root: &Path,
    destination: &Path,
) -> Result<OvenAdmittedRuntimeFoundationAsset, OvenRustcError> {
    let destination_parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let destination_parent = canonical_directory(destination_parent, "runtime foundation asset destination parent")?;
    let destination_name = destination.file_name().ok_or_else(|| {
        runtime_foundation_invalid(
            "runtime foundation asset destination",
            "must name one output directory below its parent",
        )
    })?;
    let destination = destination_parent.join(destination_name);
    // Do not rehash/copy a provider payload if an immutable installed root already owns this name.
    require_absent_runtime_foundation_destination(&destination)?;

    let mut asset = asset;
    canonicalize_runtime_foundation_asset_facts(&mut asset.foundation, &mut asset.providers)?;
    let validated = asset.clone().validated()?;
    let source_foundation_root = canonical_directory(source_foundation_root, "runtime foundation source root")?;
    let toolchain_root = canonical_directory(toolchain_root, "runtime foundation toolchain root")?;
    let owner_roots =
        runtime_foundation_asset_owner_roots(&validated, source_foundation_root.clone(), toolchain_root.clone())?;
    // Verify the source material and reject unsupported provider/native-link facts before creating any output.
    let _ = validated.materialize_for_publication(&owner_roots)?;

    let members = runtime_foundation_asset_member_paths(&validated)?;
    let staging = create_runtime_foundation_asset_staging_directory(&destination)?;
    let result = (|| {
        copy_runtime_foundation_asset_members(&source_foundation_root, &staging, &members)?;
        write_runtime_foundation_asset_descriptor(&staging, &asset)?;

        // Re-admission after copying closes the source-to-stage race and proves the staged root is complete before
        // the rename makes it visible to a normal command.
        let staged = admit_runtime_foundation_asset_for_publication(&staging, &toolchain_root)?;
        let _ = staged.materialize_asset_for_publication()?;
        require_absent_runtime_foundation_destination(&destination)?;
        fs::rename(&staging, &destination).map_err(|source| OvenRustcError::Io {
            path: destination.clone(),
            source,
        })?;
        admit_runtime_foundation_asset_for_publication(&destination, &toolchain_root)
    })();
    if result.is_err() && staging.exists() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

/// Require that a publisher never replaces an already installed foundation asset.
fn require_absent_runtime_foundation_destination(destination: &Path) -> Result<(), OvenRustcError> {
    match fs::symlink_metadata(destination) {
        Ok(_) => Err(runtime_foundation_invalid(
            "runtime foundation asset destination",
            format!("{} already exists", destination.display()),
        )),
        Err(source) if source.kind() == ErrorKind::NotFound => Ok(()),
        Err(source) => Err(OvenRustcError::Io {
            path: destination.to_path_buf(),
            source,
        }),
    }
}

/// Create a sibling private directory so `rename` is atomic within the installed-assets filesystem.
fn create_runtime_foundation_asset_staging_directory(destination: &Path) -> Result<PathBuf, OvenRustcError> {
    let parent = destination.parent().ok_or_else(|| {
        runtime_foundation_invalid(
            "runtime foundation asset destination",
            "has no parent directory for atomic publication",
        )
    })?;
    let name = destination.file_name().ok_or_else(|| {
        runtime_foundation_invalid(
            "runtime foundation asset destination",
            "has no final directory name for staging",
        )
    })?;
    let name = name.to_string_lossy();
    for attempt in 0..128_u32 {
        let staging = parent.join(format!(".{name}.staging-{}-{attempt}", std::process::id()));
        match fs::create_dir(&staging) {
            Ok(()) => return Ok(staging),
            Err(source) if source.kind() == ErrorKind::AlreadyExists => continue,
            Err(source) => {
                return Err(OvenRustcError::Io { path: staging, source });
            }
        }
    }
    Err(runtime_foundation_invalid(
        "runtime foundation asset destination",
        "could not reserve a private staging directory",
    ))
}

/// Copy exactly the foundation-owned physical members named by one validated descriptor.
fn copy_runtime_foundation_asset_members(
    source_root: &Path,
    staging_root: &Path,
    members: &RuntimeFoundationAssetMemberCatalog,
) -> Result<(), OvenRustcError> {
    for directory in &members.directories {
        let destination = staging_root.join(directory);
        fs::create_dir_all(&destination).map_err(|source| OvenRustcError::Io {
            path: destination,
            source,
        })?;
    }
    for relative_path in &members.files {
        if relative_path == OVEN_RUNTIME_FOUNDATION_ASSET_FILENAME {
            continue;
        }
        let source = safe_path(source_root, relative_path, "runtime foundation source member")?;
        let source = verified_regular_file(&source, "runtime foundation source member")?;
        let destination = staging_root.join(relative_path);
        let parent = destination.parent().ok_or_else(|| {
            runtime_foundation_invalid(
                "runtime foundation asset member",
                format!("{relative_path} has no parent directory"),
            )
        })?;
        fs::create_dir_all(parent).map_err(|source| OvenRustcError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
        fs::copy(&source, &destination).map_err(|source| OvenRustcError::Io {
            path: destination.clone(),
            source,
        })?;
        let _ = verified_regular_file(&destination, "published runtime foundation member")?;
    }
    Ok(())
}

/// Write the canonical complete foundation descriptor only after all declared payload members reached staging.
fn write_runtime_foundation_asset_descriptor(
    staging_root: &Path,
    asset: &OvenRuntimeFoundationAsset,
) -> Result<(), OvenRustcError> {
    let bytes = serde_json::to_vec(asset).map_err(|error| {
        runtime_foundation_invalid(
            "runtime foundation descriptor",
            format!("cannot encode canonical asset: {error}"),
        )
    })?;
    let descriptor = staging_root.join(OVEN_RUNTIME_FOUNDATION_ASSET_FILENAME);
    fs::write(&descriptor, bytes).map_err(|source| OvenRustcError::Io {
        path: descriptor,
        source,
    })
}

/// Load one release-owned runtime-foundation descriptor and audit its complete foundation-owned file set.
///
/// This is explicit publisher work. It reads only the named descriptor, its exact declared files, and the separately
/// supplied toolchain root. It never probes a Cargo manifest, registry/cache directory, target directory or ambient
/// compiler state. A normal command receives a later store-owned closure rather than calling this loader.
pub(crate) fn admit_runtime_foundation_asset_for_publication(
    foundation_root: &Path,
    toolchain_root: &Path,
) -> Result<OvenAdmittedRuntimeFoundationAsset, OvenRustcError> {
    let foundation_root = canonical_directory(foundation_root, "runtime foundation asset root")?;
    let toolchain_root = canonical_directory(toolchain_root, "runtime foundation toolchain root")?;
    let descriptor = verified_regular_file(
        &foundation_root.join(OVEN_RUNTIME_FOUNDATION_ASSET_FILENAME),
        "runtime foundation descriptor",
    )?;
    let bytes = fs::read(&descriptor).map_err(|source| OvenRustcError::Io {
        path: descriptor.clone(),
        source,
    })?;
    let asset = serde_json::from_slice::<OvenRuntimeFoundationAsset>(&bytes).map_err(|error| {
        runtime_foundation_invalid(
            "runtime foundation descriptor",
            format!("cannot decode {}: {error}", descriptor.display()),
        )
    })?;
    let asset = asset.validated()?;
    let owner_roots = runtime_foundation_asset_owner_roots(&asset, foundation_root.clone(), toolchain_root)?;
    audit_runtime_foundation_asset_members(&foundation_root, &asset)?;
    Ok(OvenAdmittedRuntimeFoundationAsset { asset, owner_roots })
}

/// Bind the asset's two allowed owner identities to explicit physical roots.
fn runtime_foundation_asset_owner_roots(
    asset: &ValidatedOvenRuntimeFoundationAsset,
    foundation_root: PathBuf,
    toolchain_root: PathBuf,
) -> Result<Vec<OvenSelectedRustFacetOwnerRoot>, OvenRustcError> {
    let foundation = asset.foundation();
    let graph = foundation.selected_graph().graph();
    let foundation_owner = foundation.artifact_owner();
    let toolchain_owner = graph
        .owners
        .iter()
        .find(|owner| owner.kind == OvenSelectedRustFacetOwnerKind::Toolchain)
        .map(|owner| owner.identity.clone())
        .ok_or_else(|| runtime_foundation_invalid("runtime foundation owners", "has no Toolchain owner"))?;
    let expected = BTreeSet::from([foundation_owner.to_string(), toolchain_owner.clone()]);
    let actual = graph
        .owners
        .iter()
        .map(|owner| owner.identity.clone())
        .collect::<BTreeSet<_>>();
    if actual != expected {
        return Err(runtime_foundation_invalid(
            "runtime foundation owners",
            "a release asset may name only its sealed foundation owner and its separately held Toolchain owner",
        ));
    }
    Ok(vec![
        OvenSelectedRustFacetOwnerRoot {
            identity: foundation_owner.to_string(),
            root: foundation_root,
        },
        OvenSelectedRustFacetOwnerRoot {
            identity: toolchain_owner,
            root: toolchain_root,
        },
    ])
}

/// One exact descriptor-derived regular-file and directory catalogue for a release foundation root.
#[derive(Debug, Default)]
struct RuntimeFoundationAssetMemberCatalog {
    files: BTreeSet<String>,
    directories: BTreeSet<String>,
}

/// Audit every member below the immutable foundation root against the descriptor-derived catalogue.
///
/// Source trees and artifacts carry their own digest catalogues and are rehashed by later materialization. This walk
/// establishes the complementary invariant: no undeclared file or directory, symlink or special filesystem member
/// may hide alongside them in the release payload.
fn audit_runtime_foundation_asset_members(
    foundation_root: &Path,
    asset: &ValidatedOvenRuntimeFoundationAsset,
) -> Result<(), OvenRustcError> {
    let expected = runtime_foundation_asset_member_paths(asset)?;
    let mut actual = RuntimeFoundationAssetMemberCatalog::default();
    collect_runtime_foundation_asset_members(foundation_root, foundation_root, &mut actual)?;
    if actual.files != expected.files || actual.directories != expected.directories {
        let missing_files = expected.files.difference(&actual.files).cloned().collect::<Vec<_>>();
        let extra_files = actual.files.difference(&expected.files).cloned().collect::<Vec<_>>();
        let missing_directories = expected
            .directories
            .difference(&actual.directories)
            .cloned()
            .collect::<Vec<_>>();
        let extra_directories = actual
            .directories
            .difference(&expected.directories)
            .cloned()
            .collect::<Vec<_>>();
        let mut message = Vec::new();
        if !missing_files.is_empty() {
            message.push(format!("omits declared file(s): {}", missing_files.join(", ")));
        }
        if !extra_files.is_empty() {
            message.push(format!("contains undeclared file(s): {}", extra_files.join(", ")));
        }
        if !missing_directories.is_empty() {
            message.push(format!(
                "omits declared directory(s): {}",
                missing_directories.join(", ")
            ));
        }
        if !extra_directories.is_empty() {
            message.push(format!(
                "contains undeclared directory(s): {}",
                extra_directories.join(", ")
            ));
        }
        return Err(runtime_foundation_invalid(
            "runtime foundation asset members",
            message.join("; "),
        ));
    }
    Ok(())
}

/// Derive every allowed regular file and directory below the foundation root from the selected graph and artifact
/// manifest.
fn runtime_foundation_asset_member_paths(
    asset: &ValidatedOvenRuntimeFoundationAsset,
) -> Result<RuntimeFoundationAssetMemberCatalog, OvenRustcError> {
    let foundation = asset.foundation();
    let graph = foundation.selected_graph().graph();
    let foundation_owner = foundation.artifact_owner();
    let mut members = RuntimeFoundationAssetMemberCatalog::default();
    record_runtime_foundation_asset_file(
        &mut members,
        OVEN_RUNTIME_FOUNDATION_ASSET_FILENAME,
        "runtime foundation descriptor",
    )?;
    for artifact in foundation.artifacts().declared_artifact_paths()? {
        record_runtime_foundation_asset_file(&mut members, &artifact, "runtime foundation artifact")?;
    }
    for unit in &graph.units {
        if unit.source.owner == foundation_owner {
            record_runtime_foundation_asset_directory(
                &mut members,
                &unit.source.root,
                "runtime foundation source root",
            )?;
            for source_member in &unit.source_members {
                record_runtime_foundation_asset_file(
                    &mut members,
                    &runtime_foundation_asset_member_path(
                        &unit.source.root,
                        &source_member.path,
                        "runtime foundation source member",
                    )?,
                    "runtime foundation source member",
                )?;
            }
        }
        for directory in unit
            .include_dirs
            .iter()
            .chain(unit.exclude_dirs.iter())
            .filter(|directory| directory.owner == foundation_owner)
        {
            record_runtime_foundation_asset_directory(
                &mut members,
                &directory.path,
                "runtime foundation source directory",
            )?;
        }
        for environment in unit.environment.values() {
            if let OvenSelectedRustFacetEnvironmentValue::Path { value } = environment
                && value.owner == foundation_owner
            {
                // Foundation-owned path environment values are directories in v1 (such as OUT_DIR). A file-valued
                // environment input needs its own digest-bearing schema rather than becoming an untracked exception.
                record_runtime_foundation_asset_directory(
                    &mut members,
                    &value.path,
                    "runtime foundation environment directory",
                )?;
            }
        }
        for generated in &unit.generated_inputs {
            if generated.source.owner == foundation_owner {
                record_runtime_foundation_asset_file(
                    &mut members,
                    &generated.source.path,
                    "runtime foundation generated member",
                )?;
            }
        }
    }
    for record in asset.providers.values() {
        let package = match &record.declaration {
            OvenRuntimeFoundationProviderDeclaration::NoBuildScript { package }
            | OvenRuntimeFoundationProviderDeclaration::BuildScript { package, .. } => package,
        };
        if package.root.owner != foundation_owner {
            continue;
        }
        record_runtime_foundation_asset_directory(
            &mut members,
            &package.root.path,
            "runtime foundation provider source root",
        )?;
        record_runtime_foundation_asset_file(
            &mut members,
            &runtime_foundation_asset_member_path(
                &package.root.path,
                &package.manifest.path,
                "runtime foundation provider manifest",
            )?,
            "runtime foundation provider manifest",
        )?;
        for source_member in &package.members {
            record_runtime_foundation_asset_file(
                &mut members,
                &runtime_foundation_asset_member_path(
                    &package.root.path,
                    &source_member.path,
                    "runtime foundation provider source member",
                )?,
                "runtime foundation provider source member",
            )?;
        }
    }
    Ok(members)
}

/// Join a source-tree root and one source member while preserving the portable foundation-root path vocabulary.
fn runtime_foundation_asset_member_path(
    source_root: &str,
    member: &str,
    kind: &'static str,
) -> Result<String, OvenRustcError> {
    let path = if source_root == "." {
        member.to_string()
    } else {
        format!("{source_root}/{member}")
    };
    normalized_relative_path(&path, kind)
}

/// Add one declared regular file and every required parent directory to the exact asset catalogue.
fn record_runtime_foundation_asset_file(
    members: &mut RuntimeFoundationAssetMemberCatalog,
    path: &str,
    kind: &'static str,
) -> Result<(), OvenRustcError> {
    let path = normalized_relative_path(path, kind)?;
    record_runtime_foundation_asset_parent_directories(members, &path)?;
    members.files.insert(path);
    Ok(())
}

/// Add one declared directory and every parent directory to the exact asset catalogue.
fn record_runtime_foundation_asset_directory(
    members: &mut RuntimeFoundationAssetMemberCatalog,
    path: &str,
    kind: &'static str,
) -> Result<(), OvenRustcError> {
    if path == "." {
        return Ok(());
    }
    let path = normalized_relative_path(path, kind)?;
    record_runtime_foundation_asset_parent_directories(members, &path)?;
    members.directories.insert(path);
    Ok(())
}

/// Add every non-root parent of a portable path to the exact asset directory catalogue.
fn record_runtime_foundation_asset_parent_directories(
    members: &mut RuntimeFoundationAssetMemberCatalog,
    path: &str,
) -> Result<(), OvenRustcError> {
    let mut current = PathBuf::from(path);
    while let Some(directory) = current.parent() {
        if directory.as_os_str().is_empty() {
            break;
        }
        let directory = normalized_relative_path(&directory.to_string_lossy(), "runtime foundation asset directory")?;
        members.directories.insert(directory.clone());
        current = PathBuf::from(directory);
    }
    Ok(())
}

/// Enumerate an asset root without following links or treating unrecorded directory contents as benign.
fn collect_runtime_foundation_asset_members(
    root: &Path,
    directory: &Path,
    members: &mut RuntimeFoundationAssetMemberCatalog,
) -> Result<(), OvenRustcError> {
    let mut entries = fs::read_dir(directory)
        .map_err(|source| OvenRustcError::Io {
            path: directory.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| OvenRustcError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|source| OvenRustcError::Io {
            path: path.clone(),
            source,
        })?;
        if metadata.file_type().is_symlink() {
            return Err(runtime_foundation_invalid(
                "runtime foundation asset members",
                format!("contains symlink {}", path.display()),
            ));
        }
        if metadata.is_dir() {
            let relative = path.strip_prefix(root).map_err(|_| {
                runtime_foundation_invalid(
                    "runtime foundation asset members",
                    format!("directory {} escaped its foundation root", path.display()),
                )
            })?;
            let relative = normalized_relative_path(&relative.to_string_lossy(), "runtime foundation asset directory")?;
            if !members.directories.insert(relative) {
                return Err(runtime_foundation_invalid(
                    "runtime foundation asset members",
                    "contains one portable directory path more than once",
                ));
            }
            collect_runtime_foundation_asset_members(root, &path, members)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(runtime_foundation_invalid(
                "runtime foundation asset members",
                format!("contains non-regular member {}", path.display()),
            ));
        }
        let relative = path.strip_prefix(root).map_err(|_| {
            runtime_foundation_invalid(
                "runtime foundation asset members",
                format!("member {} escaped its foundation root", path.display()),
            )
        })?;
        let relative = normalized_relative_path(&relative.to_string_lossy(), "runtime foundation asset member")?;
        if !members.files.insert(relative) {
            return Err(runtime_foundation_invalid(
                "runtime foundation asset members",
                "contains one portable member path more than once",
            ));
        }
    }
    Ok(())
}

/// Derive the content identity of a release asset without confusing it with a compiled-unit output identity.
fn runtime_foundation_asset_identity(
    foundation: &OvenRuntimeFoundation,
    providers: &[OvenRuntimeFoundationProviderRecord],
) -> Result<String, OvenRustcError> {
    let mut foundation = foundation.clone();
    let mut providers = providers.to_vec();
    canonicalize_runtime_foundation_asset_facts(&mut foundation, &mut providers)?;
    let bytes = serde_json::to_vec(&(
        "incan.oven.runtime-foundation-asset/3",
        OVEN_RUNTIME_FOUNDATION_ASSET_SCHEMA_VERSION,
        &foundation,
        &providers,
    ))
    .map_err(|error| {
        runtime_foundation_invalid(
            "runtime foundation asset identity",
            format!("cannot encode canonical asset facts: {error}"),
        )
    })?;
    Ok(digest_bytes(&bytes))
}

/// Normalize presentation-order fields before they become a release-asset identity input.
///
/// The selected graph already has a canonical form, while foundation execution policy and provider records are
/// semantically maps keyed by selected identity. Their arrival order must not manufacture a second foundation or
/// defeat reuse. Artifact-manifest ordering is deliberately left intact because its search-path order can affect the
/// compiler invocation and is therefore part of the manifest's observable contract.
fn canonicalize_runtime_foundation_asset_facts(
    foundation: &mut OvenRuntimeFoundation,
    providers: &mut [OvenRuntimeFoundationProviderRecord],
) -> Result<(), OvenRustcError> {
    foundation.selected_graph = foundation
        .selected_graph
        .clone()
        .validated()
        .map_err(|error| runtime_foundation_invalid("runtime foundation selected graph", error.to_string()))?
        .graph()
        .clone();
    foundation
        .units
        .sort_by(|left, right| left.selected_identity.cmp(&right.selected_identity));
    providers.sort_by(|left, right| left.selected_identity.cmp(&right.selected_identity));
    for provider in providers {
        match &mut provider.declaration {
            OvenRuntimeFoundationProviderDeclaration::NoBuildScript { package } => package.members.sort(),
            OvenRuntimeFoundationProviderDeclaration::BuildScript {
                package,
                host_dependencies,
                ..
            } => {
                package.members.sort();
                host_dependencies
                    .sort_by(|left, right| left.alias.cmp(&right.alias).then_with(|| left.unit.cmp(&right.unit)));
            }
        }
        if let OvenRuntimeFoundationProviderState::Captured { receipt } = &mut provider.state {
            receipt
                .effects
                .generated_inputs
                .sort_by(|left, right| left.name.cmp(&right.name).then_with(|| left.source.cmp(&right.source)));
            receipt.effects.emitted_cfg.sort();
            receipt.effects.checked_cfg.sort();
            receipt.effects.rerun_paths.sort();
            receipt.effects.rerun_environment.sort();
        }
    }
    Ok(())
}

/// Seal every captured receipt from its complete typed effect record after canonical presentation ordering is fixed.
///
/// This runs only on the publisher-side constructor. Admission recomputes and compares these values; it must never
/// repair a descriptor whose effect record and asserted receipt identity disagree.
fn seal_runtime_foundation_provider_receipts(
    providers: &mut [OvenRuntimeFoundationProviderRecord],
) -> Result<(), OvenRustcError> {
    for record in providers {
        let OvenRuntimeFoundationProviderState::Captured { receipt } = &mut record.state else {
            continue;
        };
        if receipt.schema_version != OVEN_RUNTIME_FOUNDATION_PROVIDER_RECEIPT_SCHEMA_VERSION {
            return Err(runtime_foundation_invalid(
                "runtime foundation provider receipt schema",
                format!(
                    "expected schema {OVEN_RUNTIME_FOUNDATION_PROVIDER_RECEIPT_SCHEMA_VERSION}, found {}",
                    receipt.schema_version
                ),
            ));
        }
        receipt.effect_digest = runtime_foundation_provider_effect_digest(&receipt.effects)?;
        receipt.provider_receipt_identity = runtime_foundation_provider_receipt_identity(
            &record.selected_identity,
            &record.declaration,
            &receipt.effects,
        )?;
    }
    Ok(())
}

/// Derive the digest of the entire normalized build-script effect vocabulary.
fn runtime_foundation_provider_effect_digest(
    effects: &OvenRuntimeFoundationProviderEffects,
) -> Result<String, OvenRustcError> {
    let bytes = serde_json::to_vec(&(
        "incan.oven.runtime-foundation-provider-effects/1",
        OVEN_RUNTIME_FOUNDATION_PROVIDER_RECEIPT_SCHEMA_VERSION,
        effects,
    ))
    .map_err(|error| {
        runtime_foundation_invalid(
            "runtime foundation provider effect digest",
            format!("cannot encode canonical provider effects: {error}"),
        )
    })?;
    Ok(digest_bytes(&bytes))
}

/// Derive one provider receipt identity from its selected unit, declared script input and complete effect record.
fn runtime_foundation_provider_receipt_identity(
    selected_identity: &str,
    declaration: &OvenRuntimeFoundationProviderDeclaration,
    effects: &OvenRuntimeFoundationProviderEffects,
) -> Result<String, OvenRustcError> {
    let bytes = serde_json::to_vec(&(
        "incan.oven.runtime-foundation-provider-receipt/1",
        OVEN_RUNTIME_FOUNDATION_PROVIDER_RECEIPT_SCHEMA_VERSION,
        selected_identity,
        declaration,
        effects,
    ))
    .map_err(|error| {
        runtime_foundation_invalid(
            "runtime foundation provider receipt identity",
            format!("cannot encode canonical provider receipt: {error}"),
        )
    })?;
    Ok(digest_bytes(&bytes))
}

/// Parse a bounded build-script stdout stream without letting unsupported output become an unrecorded compiler fact.
///
/// This recognizes both Cargo's legacy `cargo:` spelling and its current `cargo::` spelling, but it does not invoke
/// Cargo or infer a dependency graph. Raw environment values remain in this short-lived result until a later caller
/// proves they equal the selected graph's declared environment representation.
fn parse_runtime_foundation_provider_directives(
    stdout: &[u8],
) -> Result<OvenRuntimeFoundationProviderDirectives, OvenRustcError> {
    if stdout.len() > OVEN_RUNTIME_FOUNDATION_PROVIDER_STDOUT_LIMIT {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider stdout",
            format!(
                "exceeds the {} byte provider-output limit",
                OVEN_RUNTIME_FOUNDATION_PROVIDER_STDOUT_LIMIT
            ),
        ));
    }
    let stdout = std::str::from_utf8(stdout)
        .map_err(|_| runtime_foundation_invalid("runtime foundation provider stdout", "contains non-UTF-8 bytes"))?;
    let mut emitted_cfg = Vec::new();
    let mut checked_cfg = Vec::new();
    let mut emitted_environment = BTreeMap::new();
    let mut rerun_paths = Vec::new();
    let mut rerun_environment = Vec::new();
    let mut native_directives = BTreeSet::new();
    for line in stdout.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        let directive = line
            .strip_prefix("cargo::")
            .or_else(|| line.strip_prefix("cargo:"))
            .ok_or_else(|| {
                runtime_foundation_invalid(
                    "runtime foundation provider stdout",
                    "contains a non-directive output line",
                )
            })?;
        let (name, value) = directive.split_once('=').ok_or_else(|| {
            runtime_foundation_invalid("runtime foundation provider directive", "has no `=` separator")
        })?;
        match name {
            "rustc-cfg" => {
                require_nonempty_runtime_foundation_provider_directive_value(name, value)?;
                emitted_cfg.push(value.to_string());
            }
            "rustc-check-cfg" => {
                require_nonempty_runtime_foundation_provider_directive_value(name, value)?;
                checked_cfg.push(value.to_string());
            }
            "rustc-env" => {
                let (environment_name, environment_value) = value.split_once('=').ok_or_else(|| {
                    runtime_foundation_invalid(
                        "runtime foundation provider rustc-env",
                        "has no environment-value separator",
                    )
                })?;
                if environment_name.trim().is_empty()
                    || environment_name.bytes().any(|byte| byte == b'=' || byte == b'\0')
                    || emitted_environment
                        .insert(environment_name.to_string(), environment_value.to_string())
                        .is_some()
                {
                    return Err(runtime_foundation_invalid(
                        "runtime foundation provider rustc-env",
                        "has an empty, malformed or repeated environment name",
                    ));
                }
            }
            "rerun-if-changed" => {
                require_nonempty_runtime_foundation_provider_directive_value(name, value)?;
                rerun_paths.push(value.to_string());
            }
            "rerun-if-env-changed" => {
                require_nonempty_runtime_foundation_provider_directive_value(name, value)?;
                rerun_environment.push(value.to_string());
            }
            "rustc-link-lib" | "rustc-link-search" | "rustc-link-arg" | "rustc-cdylib-link-arg" => {
                require_nonempty_runtime_foundation_provider_directive_value(name, value)?;
                native_directives.insert(name);
            }
            _ => {
                return Err(runtime_foundation_invalid(
                    "runtime foundation provider directive",
                    format!("does not support `{name}`"),
                ));
            }
        }
    }
    let native_link = if native_directives.is_empty() {
        OvenRuntimeFoundationNativeLinkState::NoNativeLink
    } else {
        OvenRuntimeFoundationNativeLinkState::Unsupported {
            reason: format!(
                "provider emitted native-link directive(s): {}",
                native_directives.into_iter().collect::<Vec<_>>().join(", ")
            ),
        }
    };
    Ok(OvenRuntimeFoundationProviderDirectives {
        emitted_cfg,
        checked_cfg,
        emitted_environment,
        rerun_paths,
        rerun_environment,
        native_link,
    })
}

/// Reject an absent or whitespace-only directive value without including the possibly sensitive value in diagnostics.
fn require_nonempty_runtime_foundation_provider_directive_value(
    directive: &str,
    value: &str,
) -> Result<(), OvenRustcError> {
    if value.trim().is_empty() {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider directive",
            format!("`{directive}` has an empty value"),
        ));
    }
    Ok(())
}

/// Bind parsed build-script output to the selected unit and its admitted physical environment.
///
/// This is deliberately a comparison, never an inference. The caller supplies generated-input facts from its owned
/// provider output staging and the physical environment from selected-unit materialization. Every retained value is
/// converted back to the selected graph's portable representation before it can enter a receipt.
fn bind_runtime_foundation_provider_directives(
    unit: &OvenSelectedRustFacetUnit,
    declaration: &OvenRuntimeFoundationProviderDeclaration,
    materialized_environment: &BTreeMap<String, OvenMaterializedRustFacetEnvironmentValue>,
    generated_inputs: Vec<OvenSelectedRustFacetGeneratedInput>,
    directives: OvenRuntimeFoundationProviderDirectives,
) -> Result<OvenRuntimeFoundationProviderEffects, OvenRustcError> {
    let OvenRuntimeFoundationProviderDeclaration::BuildScript { package, .. } = declaration else {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider directives",
            format!("unit {} has no declared build-script source closure", unit.crate_name),
        ));
    };
    if generated_inputs != unit.generated_inputs {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider generated inputs",
            format!(
                "unit {} provider-generated members do not exactly match the selected graph",
                unit.crate_name
            ),
        ));
    }
    let mut emitted_environment = BTreeMap::new();
    for (name, value) in directives.emitted_environment {
        let selected = unit.environment.get(&name).ok_or_else(|| {
            runtime_foundation_invalid(
                "runtime foundation provider environment",
                format!(
                    "unit {} emitted an environment name absent from the selected graph",
                    unit.crate_name
                ),
            )
        })?;
        let materialized = materialized_environment.get(&name).ok_or_else(|| {
            runtime_foundation_invalid(
                "runtime foundation provider environment",
                format!(
                    "unit {} has no admitted physical environment for its emitted name",
                    unit.crate_name
                ),
            )
        })?;
        let matches = match (selected, materialized) {
            (
                OvenSelectedRustFacetEnvironmentValue::Text { value: expected },
                OvenMaterializedRustFacetEnvironmentValue::Text(materialized),
            ) => expected == &value && materialized == expected,
            (
                OvenSelectedRustFacetEnvironmentValue::Path { .. },
                OvenMaterializedRustFacetEnvironmentValue::Path(materialized),
            ) => materialized.to_str() == Some(value.as_str()),
            (OvenSelectedRustFacetEnvironmentValue::SensitiveDigest { .. }, _) => {
                return Err(runtime_foundation_invalid(
                    "runtime foundation provider environment",
                    format!(
                        "unit {} emitted a redacted environment value without a separately admitted value provider",
                        unit.crate_name
                    ),
                ));
            }
            _ => false,
        };
        if !matches {
            return Err(runtime_foundation_invalid(
                "runtime foundation provider environment",
                format!(
                    "unit {} emitted an environment value that differs from its admitted selection",
                    unit.crate_name
                ),
            ));
        }
        emitted_environment.insert(name, selected.clone());
    }
    let mut effects = OvenRuntimeFoundationProviderEffects {
        generated_inputs,
        emitted_cfg: directives.emitted_cfg,
        checked_cfg: directives.checked_cfg,
        emitted_environment,
        rerun_paths: directives.rerun_paths,
        rerun_environment: directives.rerun_environment,
        native_link: directives.native_link,
    };
    effects.emitted_cfg.sort();
    effects.checked_cfg.sort();
    effects.rerun_paths.sort();
    effects.rerun_environment.sort();
    if effects.emitted_cfg.windows(2).any(|pair| pair[0] >= pair[1])
        || effects
            .emitted_cfg
            .iter()
            .any(|cfg| cfg.trim().is_empty() || !unit.cfg.contains(cfg))
    {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider cfg",
            format!(
                "unit {} emitted cfg values absent from or repeated in the selected graph",
                unit.crate_name
            ),
        ));
    }
    if effects.checked_cfg.windows(2).any(|pair| pair[0] >= pair[1])
        || effects.checked_cfg.iter().any(|cfg| cfg.trim().is_empty())
    {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider checked cfg",
            format!("unit {} emitted empty or repeated check-cfg values", unit.crate_name),
        ));
    }
    if effects.rerun_paths.windows(2).any(|pair| pair[0] >= pair[1])
        || effects
            .rerun_paths
            .iter()
            .any(|path| !runtime_foundation_provider_package_contains_path(package, path))
    {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider rerun paths",
            format!(
                "unit {} emitted rerun paths absent from its declared package source members",
                unit.crate_name
            ),
        ));
    }
    if effects.rerun_environment.windows(2).any(|pair| pair[0] >= pair[1])
        || effects
            .rerun_environment
            .iter()
            .any(|name| name.trim().is_empty() || !unit.environment.contains_key(name))
    {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider rerun environment",
            format!(
                "unit {} emitted rerun environment names absent from the selected graph",
                unit.crate_name
            ),
        ));
    }
    Ok(effects)
}

/// Check that each selected unit has exactly one authoritative provider-effect classification.
fn validate_runtime_foundation_provider_records(
    foundation: &ValidatedOvenRuntimeFoundation,
    records: Vec<OvenRuntimeFoundationProviderRecord>,
) -> Result<BTreeMap<String, OvenRuntimeFoundationProviderRecord>, OvenRustcError> {
    let units = foundation
        .selected_graph()
        .graph()
        .units
        .iter()
        .map(|unit| (unit.identity.as_str(), unit))
        .collect::<BTreeMap<_, _>>();
    let mut providers = BTreeMap::new();
    for record in records {
        validate_sha256_identity(&record.selected_identity, "runtime foundation provider unit identity")?;
        let unit = units.get(record.selected_identity.as_str()).ok_or_else(|| {
            runtime_foundation_invalid(
                "runtime foundation providers",
                format!("names unknown selected unit {}", record.selected_identity),
            )
        })?;
        validate_runtime_foundation_provider_record(unit, &units, &record)?;
        if providers.insert(record.selected_identity.clone(), record).is_some() {
            return Err(runtime_foundation_invalid(
                "runtime foundation providers",
                "declares one selected unit more than once",
            ));
        }
    }
    let expected = units.keys().copied().collect::<BTreeSet<_>>();
    let actual = providers.keys().map(String::as_str).collect::<BTreeSet<_>>();
    if expected != actual {
        let missing = expected.difference(&actual).copied().collect::<Vec<_>>();
        let extra = actual.difference(&expected).copied().collect::<Vec<_>>();
        let mut message = Vec::new();
        if !missing.is_empty() {
            message.push(format!("omits selected unit provider state(s): {}", missing.join(", ")));
        }
        if !extra.is_empty() {
            message.push(format!(
                "names unknown selected unit provider state(s): {}",
                extra.join(", ")
            ));
        }
        return Err(runtime_foundation_invalid(
            "runtime foundation providers",
            message.join("; "),
        ));
    }
    Ok(providers)
}

/// Ensure one provider record agrees with the one shared selected graph rather than recreating unit facts.
fn validate_runtime_foundation_provider_record(
    unit: &OvenSelectedRustFacetUnit,
    units: &BTreeMap<&str, &OvenSelectedRustFacetUnit>,
    record: &OvenRuntimeFoundationProviderRecord,
) -> Result<(), OvenRustcError> {
    validate_runtime_foundation_provider_declaration(unit, units, &record.declaration)?;
    match (&record.declaration, &record.state) {
        (
            OvenRuntimeFoundationProviderDeclaration::NoBuildScript { .. },
            OvenRuntimeFoundationProviderState::NoProvider,
        ) => {
            if unit.generated_inputs.is_empty() {
                Ok(())
            } else {
                Err(runtime_foundation_invalid(
                    "runtime foundation providers",
                    format!(
                        "unit {} names generated input(s) but declares no build script",
                        unit.crate_name
                    ),
                ))
            }
        }
        (OvenRuntimeFoundationProviderDeclaration::NoBuildScript { .. }, _) => Err(runtime_foundation_invalid(
            "runtime foundation providers",
            format!(
                "unit {} declares no build script but carries provider execution facts",
                unit.crate_name
            ),
        )),
        (
            OvenRuntimeFoundationProviderDeclaration::BuildScript { .. },
            OvenRuntimeFoundationProviderState::NoProvider,
        ) => Err(runtime_foundation_invalid(
            "runtime foundation providers",
            format!(
                "unit {} declares a build script but omits its provider execution result",
                unit.crate_name
            ),
        )),
        (
            OvenRuntimeFoundationProviderDeclaration::BuildScript { .. },
            OvenRuntimeFoundationProviderState::Captured { receipt },
        ) => validate_runtime_foundation_provider_receipt(unit, record, receipt),
        (
            OvenRuntimeFoundationProviderDeclaration::BuildScript { .. },
            OvenRuntimeFoundationProviderState::Unsupported { reason },
        ) => {
            if reason.trim().is_empty() {
                return Err(runtime_foundation_invalid(
                    "runtime foundation providers",
                    format!("unit {} has an empty unsupported-provider reason", unit.crate_name),
                ));
            }
            Ok(())
        }
    }
}

/// Validate one retained package source against its existing selected compiler unit.
fn validate_runtime_foundation_provider_package_source(
    unit: &OvenSelectedRustFacetUnit,
    package: &OvenRuntimeFoundationProviderPackageSource,
) -> Result<(), OvenRustcError> {
    if package.root.owner != unit.source.owner {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider package root",
            format!("unit {} package root has a different selected owner", unit.crate_name),
        ));
    }
    let package_root = portable_provider_package_root_components(&package.root.path)?;
    let compiler_root = portable_provider_package_root_components(&unit.source.root)?;
    if !compiler_root.starts_with(&package_root) {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider package root",
            format!(
                "unit {} package root does not contain its compiler root",
                unit.crate_name
            ),
        ));
    }
    if package.manifest.path != "Cargo.toml" {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider manifest",
            format!(
                "unit {} manifest must be Cargo.toml relative to its package root",
                unit.crate_name
            ),
        ));
    }
    validate_sha256_identity(&package.manifest.digest, "runtime foundation provider manifest digest")?;
    let _ = selected_graph_source_digest(&package.members)
        .map_err(|error| runtime_foundation_invalid("runtime foundation provider source members", error.to_string()))?;
    if package.members.windows(2).any(|pair| pair[0].path >= pair[1].path) {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider source members",
            format!(
                "unit {} provider package members must be sorted and unique",
                unit.crate_name
            ),
        ));
    }
    if package
        .members
        .iter()
        .any(|member| member.path == package.manifest.path)
    {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider manifest",
            format!("unit {} repeats its manifest in package members", unit.crate_name),
        ));
    }
    let compiler_root_relative_to_package = &compiler_root[package_root.len()..];
    for unit_member in &unit.source_members {
        let package_member_path = compiler_root_relative_to_package
            .iter()
            .copied()
            .chain(std::iter::once(unit_member.path.as_str()))
            .collect::<Vec<_>>()
            .join("/");
        if package_member_path == package.manifest.path {
            if unit_member.digest != package.manifest.digest {
                return Err(runtime_foundation_invalid(
                    "runtime foundation provider manifest",
                    format!(
                        "unit {} gives compiler-visible manifest bytes conflicting with its package manifest",
                        unit.crate_name
                    ),
                ));
            }
            continue;
        }
        match package.members.iter().find(|member| member.path == package_member_path) {
            Some(package_member) if package_member.digest == unit_member.digest => {}
            Some(_) => {
                return Err(runtime_foundation_invalid(
                    "runtime foundation provider source members",
                    format!(
                        "unit {} gives compiler-visible source member {} conflicting package bytes",
                        unit.crate_name, package_member_path
                    ),
                ));
            }
            None => {
                return Err(runtime_foundation_invalid(
                    "runtime foundation provider source members",
                    format!(
                        "unit {} omits compiler-visible source member {} from its package inventory",
                        unit.crate_name, package_member_path
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Return whether one provider rerun observation names a byte already retained by the package declaration.
///
/// The manifest has its own typed field so it cannot be confused with a generic source member, but it remains a
/// real package-root file that a build script may read or explicitly request for rerun observation.
fn runtime_foundation_provider_package_contains_path(
    package: &OvenRuntimeFoundationProviderPackageSource,
    path: &str,
) -> bool {
    path == package.manifest.path || package.members.iter().any(|member| member.path == path)
}

/// Split one portable package root into normalized components without letting host-native path syntax change scope.
fn portable_provider_package_root_components(value: &str) -> Result<Vec<&str>, OvenRustcError> {
    if value == "." {
        return Ok(Vec::new());
    }
    if value.trim().is_empty()
        || value.starts_with('/')
        || value.contains('\\')
        || value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == ".." || part.contains(':'))
    {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider package root",
            "must be a normalized portable owner-relative path",
        ));
    }
    Ok(value.split('/').collect())
}

/// Validate a producer-declared build script entirely against already selected source and host units.
fn validate_runtime_foundation_provider_declaration(
    unit: &OvenSelectedRustFacetUnit,
    units: &BTreeMap<&str, &OvenSelectedRustFacetUnit>,
    declaration: &OvenRuntimeFoundationProviderDeclaration,
) -> Result<(), OvenRustcError> {
    let (package, entrypoint, digest, edition, host_dependencies) = match declaration {
        OvenRuntimeFoundationProviderDeclaration::NoBuildScript { package } => {
            return validate_runtime_foundation_provider_package_source(unit, package);
        }
        OvenRuntimeFoundationProviderDeclaration::BuildScript {
            package,
            entrypoint,
            digest,
            edition,
            host_dependencies,
        } => (package, entrypoint, digest, edition, host_dependencies),
    };
    validate_runtime_foundation_provider_package_source(unit, package)?;
    let package_members = &package.members;
    validate_sha256_identity(digest, "runtime foundation provider build-script digest")?;
    let member = package_members
        .iter()
        .find(|member| member.path == *entrypoint)
        .ok_or_else(|| {
            runtime_foundation_invalid(
                "runtime foundation provider build-script entrypoint",
                format!(
                    "unit {} names a source member absent from the declared package inventory",
                    unit.crate_name
                ),
            )
        })?;
    if member.digest != *digest {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider build-script digest",
            format!(
                "unit {} build-script digest differs from its declared provider source member",
                unit.crate_name
            ),
        ));
    }
    if edition != &unit.edition {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider build-script edition",
            format!(
                "unit {} build-script edition differs from its selected unit",
                unit.crate_name
            ),
        ));
    }
    validate_runtime_foundation_provider_host_dependencies(unit, units, host_dependencies)
}

/// Bind exact host-domain build dependencies to already selected direct edges.
fn validate_runtime_foundation_provider_host_dependencies(
    unit: &OvenSelectedRustFacetUnit,
    units: &BTreeMap<&str, &OvenSelectedRustFacetUnit>,
    host_dependencies: &[OvenRuntimeFoundationProviderHostDependency],
) -> Result<(), OvenRustcError> {
    if host_dependencies
        .windows(2)
        .any(|pair| pair[0].alias > pair[1].alias || (pair[0].alias == pair[1].alias && pair[0].unit >= pair[1].unit))
    {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider host dependencies",
            format!("unit {} host dependencies must be sorted and unique", unit.crate_name),
        ));
    }
    let mut aliases = BTreeSet::new();
    for dependency in host_dependencies {
        if dependency.alias.trim().is_empty() || !aliases.insert(dependency.alias.as_str()) {
            return Err(runtime_foundation_invalid(
                "runtime foundation provider host dependencies",
                format!(
                    "unit {} has an empty or repeated build-script dependency alias",
                    unit.crate_name
                ),
            ));
        }
        validate_sha256_identity(&dependency.unit, "runtime foundation provider host dependency unit")?;
        let dependency_unit = units.get(dependency.unit.as_str()).ok_or_else(|| {
            runtime_foundation_invalid(
                "runtime foundation provider host dependencies",
                format!("unit {} names an unknown build-script host dependency", unit.crate_name),
            )
        })?;
        if dependency_unit.domain != OvenSelectedRustFacetDomain::Host {
            return Err(runtime_foundation_invalid(
                "runtime foundation provider host dependencies",
                format!("unit {} names a non-host build-script dependency", unit.crate_name),
            ));
        }
        if dependency.unit == unit.identity {
            return Err(runtime_foundation_invalid(
                "runtime foundation provider host dependencies",
                format!(
                    "unit {} cannot depend on itself while compiling its build script",
                    unit.crate_name
                ),
            ));
        }
        if !unit
            .dependencies
            .iter()
            .any(|selected| selected.alias == dependency.alias && selected.unit == dependency.unit)
        {
            return Err(runtime_foundation_invalid(
                "runtime foundation provider host dependencies",
                format!(
                    "unit {} names a host dependency absent from its selected direct dependency edges",
                    unit.crate_name
                ),
            ));
        }
    }
    Ok(())
}

/// Validate a complete typed receipt against both its own canonical hashes and the selected graph facts it projects.
fn validate_runtime_foundation_provider_receipt(
    unit: &OvenSelectedRustFacetUnit,
    record: &OvenRuntimeFoundationProviderRecord,
    receipt: &OvenRuntimeFoundationProviderReceipt,
) -> Result<(), OvenRustcError> {
    let OvenRuntimeFoundationProviderDeclaration::BuildScript { package, .. } = &record.declaration else {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider receipt",
            format!(
                "unit {} has a receipt without a build-script declaration",
                unit.crate_name
            ),
        ));
    };
    if receipt.schema_version != OVEN_RUNTIME_FOUNDATION_PROVIDER_RECEIPT_SCHEMA_VERSION {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider receipt schema",
            format!(
                "expected schema {OVEN_RUNTIME_FOUNDATION_PROVIDER_RECEIPT_SCHEMA_VERSION}, found {}",
                receipt.schema_version
            ),
        ));
    }
    validate_sha256_identity(
        &receipt.provider_receipt_identity,
        "runtime foundation provider receipt identity",
    )?;
    validate_sha256_identity(&receipt.effect_digest, "runtime foundation provider effect digest")?;
    let expected_effect_digest = runtime_foundation_provider_effect_digest(&receipt.effects)?;
    if receipt.effect_digest != expected_effect_digest {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider effect digest",
            format!(
                "unit {} receipt does not match its complete typed effects",
                unit.crate_name
            ),
        ));
    }
    let expected_receipt_identity =
        runtime_foundation_provider_receipt_identity(&record.selected_identity, &record.declaration, &receipt.effects)?;
    if receipt.provider_receipt_identity != expected_receipt_identity {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider receipt identity",
            format!(
                "unit {} receipt does not match its declaration and effects",
                unit.crate_name
            ),
        ));
    }
    let effects = &receipt.effects;
    if effects.generated_inputs != unit.generated_inputs {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider generated inputs",
            format!(
                "unit {} provider outputs do not exactly match its selected generated inputs",
                unit.crate_name
            ),
        ));
    }
    if effects.emitted_cfg.windows(2).any(|pair| pair[0] >= pair[1])
        || effects.emitted_cfg.iter().any(|cfg| cfg.trim().is_empty())
        || effects.emitted_cfg.iter().any(|cfg| !unit.cfg.contains(cfg))
    {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider cfg",
            format!(
                "unit {} provider cfg values must be unique and selected by the shared graph",
                unit.crate_name
            ),
        ));
    }
    if effects
        .emitted_environment
        .iter()
        .any(|(name, value)| name.trim().is_empty() || unit.environment.get(name) != Some(value))
    {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider environment",
            format!(
                "unit {} provider environment must be selected by the shared graph",
                unit.crate_name
            ),
        ));
    }
    if effects.checked_cfg.windows(2).any(|pair| pair[0] >= pair[1])
        || effects.checked_cfg.iter().any(|cfg| cfg.trim().is_empty())
    {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider checked cfg",
            format!(
                "unit {} provider checked cfg values must be unique and nonempty",
                unit.crate_name
            ),
        ));
    }
    if effects.rerun_paths.windows(2).any(|pair| pair[0] >= pair[1])
        || effects
            .rerun_paths
            .iter()
            .any(|path| !runtime_foundation_provider_package_contains_path(package, path))
    {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider rerun paths",
            format!(
                "unit {} provider rerun paths must name declared package source members",
                unit.crate_name
            ),
        ));
    }
    if effects.rerun_environment.windows(2).any(|pair| pair[0] >= pair[1])
        || effects
            .rerun_environment
            .iter()
            .any(|name| name.trim().is_empty() || !unit.environment.contains_key(name))
    {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider rerun environment",
            format!(
                "unit {} provider rerun environment must be selected by the shared graph",
                unit.crate_name
            ),
        ));
    }
    if let OvenRuntimeFoundationNativeLinkState::Unsupported { reason } = &effects.native_link
        && reason.trim().is_empty()
    {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider native link",
            format!("unit {} has an empty unsupported native-link reason", unit.crate_name),
        ));
    }
    Ok(())
}

/// Derive a deterministic rebuild order from the shared selected graph without resolving any new dependency edge.
fn runtime_foundation_rebuild_order(
    graph: &OvenSelectedRustFacetGraph,
    policies: &BTreeMap<String, OvenRuntimeFoundationUnit>,
) -> Result<Vec<String>, OvenRustcError> {
    let units = graph
        .units
        .iter()
        .map(|unit| (unit.identity.as_str(), unit))
        .collect::<BTreeMap<_, _>>();
    let mut pending = policies
        .iter()
        .filter_map(|(identity, policy)| {
            matches!(policy.execution, OvenRuntimeFoundationUnitExecution::Rebuild).then_some(identity.clone())
        })
        .collect::<BTreeSet<_>>();
    let mut order = Vec::with_capacity(pending.len());
    while !pending.is_empty() {
        let ready = pending
            .iter()
            .filter_map(|identity| {
                let unit = units.get(identity.as_str())?;
                unit.dependencies
                    .iter()
                    .all(|dependency| !pending.contains(&dependency.unit))
                    .then_some(identity.clone())
            })
            .collect::<Vec<_>>();
        if ready.is_empty() {
            return Err(runtime_foundation_invalid(
                "runtime foundation rebuild graph",
                "has no compiler-owned dependency frontier",
            ));
        }
        for identity in ready {
            pending.remove(&identity);
            order.push(identity);
        }
    }
    Ok(order)
}

/// Validate one declared policy against its selected graph unit and the sealed artifact/source catalogue.
fn validate_runtime_unit_policy(
    unit: &OvenSelectedRustFacetUnit,
    policy: &OvenRuntimeFoundationUnit,
    artifact_owner: &str,
    artifacts: &OvenRustcArtifactManifest,
    declared_artifacts: &BTreeMap<String, String>,
) -> Result<(), OvenRustcError> {
    match &policy.execution {
        OvenRuntimeFoundationUnitExecution::Prebuilt { artifact } => {
            if unit.source.kind != OvenSelectedRustFacetSourceKind::Registry {
                return Err(runtime_foundation_invalid(
                    "runtime foundation prebuilt unit",
                    format!("unit {} is not registry-backed", unit.crate_name),
                ));
            }
            if artifact.crate_name != unit.crate_name {
                return Err(runtime_foundation_invalid(
                    "runtime foundation prebuilt artifact",
                    format!(
                        "unit {} maps to artifact crate {}",
                        unit.crate_name, artifact.crate_name
                    ),
                ));
            }
            if !artifacts.externs.iter().any(|declared| declared == artifact) {
                return Err(runtime_foundation_invalid(
                    "runtime foundation prebuilt artifact",
                    format!(
                        "unit {} artifact {} is not exposed as a sealed direct extern",
                        unit.crate_name, artifact.relative_path
                    ),
                ));
            }
            match declared_artifacts.get(&artifact.relative_path) {
                Some(digest) if digest == &artifact.digest => {}
                Some(_) => {
                    return Err(runtime_foundation_invalid(
                        "runtime foundation prebuilt artifact",
                        format!("artifact {} has a different declared digest", artifact.relative_path),
                    ));
                }
                None => {
                    return Err(runtime_foundation_invalid(
                        "runtime foundation prebuilt artifact",
                        format!("artifact {} is absent from the sealed manifest", artifact.relative_path),
                    ));
                }
            }
            let source_matches = artifacts
                .registry_sources
                .iter()
                .filter(|source| {
                    source.package == unit.package
                        && source.version == unit.package_version
                        && source.source.digest == unit.source.digest
                })
                .collect::<Vec<_>>();
            let [source] = source_matches.as_slice() else {
                return Err(runtime_foundation_invalid(
                    "runtime foundation prebuilt source",
                    format!(
                        "unit {} must match exactly one sealed registry-source record, found {}",
                        source_matches.len(),
                        unit.crate_name
                    ),
                ));
            };
            if unit.source.owner != artifact_owner || unit.source.root != source.source.relative_root {
                return Err(runtime_foundation_invalid(
                    "runtime foundation prebuilt source",
                    format!(
                        "unit {} does not bind its registry source to the sealed foundation owner/root",
                        unit.crate_name
                    ),
                ));
            }
            for input in &unit.generated_inputs {
                if input.source.owner != artifact_owner {
                    return Err(runtime_foundation_invalid(
                        "runtime foundation generated input",
                        format!(
                            "unit {} names generated input {} outside the sealed foundation owner",
                            unit.crate_name, input.name
                        ),
                    ));
                }
                if declared_artifacts.get(&input.source.path) != Some(&input.digest) {
                    return Err(runtime_foundation_invalid(
                        "runtime foundation generated input",
                        format!(
                            "unit {} names generated input {} absent from the sealed artifact manifest",
                            unit.crate_name, input.name
                        ),
                    ));
                }
            }
        }
        OvenRuntimeFoundationUnitExecution::Rebuild => {
            if unit.source.kind != OvenSelectedRustFacetSourceKind::Compiler {
                return Err(runtime_foundation_invalid(
                    "runtime foundation rebuild unit",
                    format!("unit {} is not compiler-owned source", unit.crate_name),
                ));
            }
            if !matches!(
                (unit.role, unit.crate_kind),
                (
                    OvenSelectedRustFacetUnitRole::Library,
                    OvenSelectedRustFacetCrateKind::Rlib
                ) | (
                    OvenSelectedRustFacetUnitRole::ProcMacro,
                    OvenSelectedRustFacetCrateKind::ProcMacro
                )
            ) {
                return Err(runtime_foundation_invalid(
                    "runtime foundation rebuild unit",
                    format!(
                        "unit {} has unsupported rebuild role {:?} / kind {:?}",
                        unit.crate_name, unit.role, unit.crate_kind
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Validate a canonical SHA-256 content identity without treating a human-readable coordinate as compiler input.
fn validate_sha256_identity(value: &str, field: &'static str) -> Result<(), OvenRustcError> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(runtime_foundation_invalid(field, "must be a SHA-256 identity"));
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(runtime_foundation_invalid(field, "has malformed SHA-256 bytes"));
    }
    Ok(())
}

/// Construct a typed runtime-foundation validation failure.
fn runtime_foundation_invalid(field: &'static str, message: impl Into<String>) -> OvenRustcError {
    OvenRustcError::InvalidInput {
        field,
        message: message.into(),
    }
}

impl OvenRuntimeFoundationProviderIntakeEvidence {
    /// Bind one source-producer record to the existing selected graph before it can become an Engine request.
    ///
    /// The caller still has to obtain package bytes from an immutable retained owner and rehash them before launch.
    /// This constructor proves the portable record's graph relationships first: it cannot name an unknown unit, a
    /// package root outside that unit's owner/compiler root, or a host dependency absent from its direct selected
    /// edges. The original declaration slot remains available only in the request sent to the source bridge.
    pub(crate) fn new(
        selected: &ValidatedOvenSelectedRustFacetGraph,
        selected_identity: String,
        package: OvenRuntimeFoundationProviderPackageSource,
        mut host_dependencies: Vec<OvenRuntimeFoundationProviderIntakeHostDependency>,
    ) -> Result<Self, OvenRustcError> {
        host_dependencies.sort_by_key(|dependency| dependency.declaration_index);
        let evidence = Self {
            selected_identity,
            package,
            host_dependencies,
        };
        evidence.validate_against(selected)?;
        Ok(evidence)
    }

    /// Return the selected unit correlation named by this retained source evidence.
    pub(crate) fn selected_identity(&self) -> &str {
        &self.selected_identity
    }

    /// Borrow the physical package evidence whose exact bytes later bind the request and publication.
    pub(crate) fn package(&self) -> &OvenRuntimeFoundationProviderPackageSource {
        &self.package
    }

    /// Validate this evidence against the one selected graph it is allowed to describe.
    fn validate_against(&self, selected: &ValidatedOvenSelectedRustFacetGraph) -> Result<(), OvenRustcError> {
        let units = runtime_foundation_selected_units(selected);
        let unit = units.get(self.selected_identity.as_str()).ok_or_else(|| {
            runtime_foundation_invalid(
                "runtime foundation provider intake",
                "names no unit in the validated selected Rust graph",
            )
        })?;
        validate_runtime_foundation_provider_package_source(unit, &self.package)?;
        let host_dependencies = self.foundation_host_dependencies()?;
        validate_runtime_foundation_provider_host_dependencies(unit, &units, &host_dependencies)
    }

    /// Project source-request slots into the final alias/unit representation after checking canonical slot order.
    fn foundation_host_dependencies(&self) -> Result<Vec<OvenRuntimeFoundationProviderHostDependency>, OvenRustcError> {
        if self
            .host_dependencies
            .windows(2)
            .any(|pair| pair[0].declaration_index >= pair[1].declaration_index)
        {
            return Err(runtime_foundation_invalid(
                "runtime foundation provider intake evidence",
                "host dependency declaration indices must be sorted and unique",
            ));
        }
        let mut dependencies = self
            .host_dependencies
            .iter()
            .map(|dependency| OvenRuntimeFoundationProviderHostDependency {
                alias: dependency.alias.clone(),
                unit: dependency.unit.clone(),
            })
            .collect::<Vec<_>>();
        dependencies.sort_by(|left, right| left.alias.cmp(&right.alias).then_with(|| left.unit.cmp(&right.unit)));
        Ok(dependencies)
    }
}

/// Encode the exact source request for one already authenticated provider-package evidence record.
///
/// This is intentionally only an encoder. It neither issues an Engine permit nor launches a child process. The
/// caller must hold the retained owner responsible for the manifest bytes and use the resulting exact request digest
/// in a dedicated one-use Engine operation. Supplying a manifest string with different bytes than the typed manifest
/// record refuses before the source layer can infer a false no-build-script declaration.
pub(crate) fn encode_runtime_foundation_provider_intake_request(
    selected: &ValidatedOvenSelectedRustFacetGraph,
    evidence: &OvenRuntimeFoundationProviderIntakeEvidence,
    manifest_source: &str,
) -> Result<Vec<u8>, OvenRustcError> {
    evidence.validate_against(selected)?;
    let actual = digest_bytes(manifest_source.as_bytes());
    if actual != evidence.package.manifest.digest {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider intake manifest",
            "source bytes do not match the retained typed manifest digest",
        ));
    }
    serde_json::to_vec(&OvenRuntimeFoundationProviderIntakeWireRequest {
        schema: OVEN_RUNTIME_FOUNDATION_PROVIDER_INTAKE_REQUEST_SCHEMA,
        request: OvenRuntimeFoundationProviderIntakeWireRequestBody {
            selected_identity: &evidence.selected_identity,
            manifest: &evidence.package.manifest,
            manifest_source,
            package_members: &evidence.package.members,
            host_dependencies: &evidence.host_dependencies,
        },
    })
    .map_err(|error| {
        runtime_foundation_invalid(
            "runtime foundation provider intake request",
            format!("cannot encode closed source request: {error}"),
        )
    })
}

/// Index the one selected graph without inventing a second dependency relation for provider intake.
fn runtime_foundation_selected_units(
    selected: &ValidatedOvenSelectedRustFacetGraph,
) -> BTreeMap<&str, &OvenSelectedRustFacetUnit> {
    selected
        .graph()
        .units
        .iter()
        .map(|unit| (unit.identity.as_str(), unit))
        .collect()
}

/// Authenticate one Incan-authored provider declaration against retained request evidence and the selected Rust graph.
///
/// The source exchange decides only how `[package].build` and the literal `build-dependencies` declarations bind to
/// an explicitly supplied inventory. This adapter does not execute a script, discover a manifest, select a child,
/// admit a path, or create provider effects. The caller must already hold a matching Engine permit and request digest;
/// this function restores roots, manifest bytes and source inventory only from the opaque host evidence, then requires
/// every returned build-script member and host dependency to match it exactly.
pub(crate) fn decode_runtime_foundation_provider_intake(
    selected: &ValidatedOvenSelectedRustFacetGraph,
    evidence: &OvenRuntimeFoundationProviderIntakeEvidence,
    response: &[u8],
) -> Result<OvenRuntimeFoundationProviderDeclaration, OvenRustcError> {
    let response =
        serde_json::from_slice::<OvenRuntimeFoundationProviderIntakeWireResponse>(response).map_err(|_| {
            runtime_foundation_invalid(
                "runtime foundation provider intake",
                "does not use the closed Incan provider-intake response schema",
            )
        })?;
    let (schema, returned_selected_identity, declaration, refused) = match response {
        OvenRuntimeFoundationProviderIntakeWireResponse::NoBuildScript {
            schema,
            selected_identity,
        } => (schema, selected_identity, None, false),
        OvenRuntimeFoundationProviderIntakeWireResponse::BuildScript {
            schema,
            selected_identity,
            declaration,
        } => (schema, selected_identity, Some(declaration), false),
        OvenRuntimeFoundationProviderIntakeWireResponse::Refused {
            schema,
            selected_identity,
            error,
        } => {
            let _ = (error.kind, error.fields);
            (schema, selected_identity, None, true)
        }
    };
    if schema != OVEN_RUNTIME_FOUNDATION_PROVIDER_INTAKE_SCHEMA {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider intake",
            "uses an unsupported source response schema",
        ));
    }
    if returned_selected_identity != evidence.selected_identity {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider intake",
            "does not match the host-requested selected unit",
        ));
    }
    evidence.validate_against(selected)?;
    let units = runtime_foundation_selected_units(selected);
    let unit = units.get(evidence.selected_identity.as_str()).ok_or_else(|| {
        runtime_foundation_invalid(
            "runtime foundation provider intake",
            "names no unit in the validated selected Rust graph",
        )
    })?;
    if refused {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider intake",
            "the source declaration was refused and cannot become a provider state",
        ));
    }
    let expected_host_dependencies = evidence.foundation_host_dependencies()?;
    let Some(declaration) = declaration else {
        if !expected_host_dependencies.is_empty() {
            return Err(runtime_foundation_invalid(
                "runtime foundation provider intake",
                "a no-build-script response cannot discard retained host dependency evidence",
            ));
        }
        return Ok(OvenRuntimeFoundationProviderDeclaration::NoBuildScript {
            package: evidence.package.clone(),
        });
    };
    let mut package_members = declaration
        .package_members
        .into_iter()
        .map(|member| OvenSelectedRustFacetSourceMember {
            path: member.path,
            digest: member.digest,
        })
        .collect::<Vec<_>>();
    package_members.sort();
    if package_members != evidence.package.members {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider intake",
            "does not return the exact host-requested package source inventory",
        ));
    }
    let mut host_dependencies = declaration
        .host_dependencies
        .into_iter()
        .map(|dependency| OvenRuntimeFoundationProviderHostDependency {
            alias: dependency.alias,
            unit: dependency.unit,
        })
        .collect::<Vec<_>>();
    host_dependencies.sort_by(|left, right| left.alias.cmp(&right.alias).then_with(|| left.unit.cmp(&right.unit)));
    if host_dependencies != expected_host_dependencies {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider intake",
            "does not return the exact host-requested build dependency bindings",
        ));
    }
    let declaration = OvenRuntimeFoundationProviderDeclaration::BuildScript {
        package: evidence.package.clone(),
        entrypoint: declaration.entrypoint.path,
        digest: declaration.entrypoint.digest,
        edition: unit.edition.clone(),
        host_dependencies,
    };
    validate_runtime_foundation_provider_declaration(unit, &units, &declaration)?;
    Ok(declaration)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    use super::*;
    use crate::oven::OvenBuildIntent;
    use crate::oven::rustc::{
        OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION, OVEN_SELECTED_RUST_FACET_GRAPH_SCHEMA_VERSION,
        OvenRustcRegistryLeaf, OvenRustcRegistrySource, OvenRustcRegistrySourcePackage, OvenRustcSupportingArtifact,
        OvenSelectedRustFacetDependency, OvenSelectedRustFacetGeneratedInput, OvenSelectedRustFacetIntent,
        OvenSelectedRustFacetOwner, OvenSelectedRustFacetOwnerKind, OvenSelectedRustFacetPath,
        OvenSelectedRustFacetPurpose, OvenSelectedRustFacetSelection, OvenSelectedRustFacetSource,
        OvenSelectedRustFacetSourceMember, OvenSelectedRustFacetTargetSpec, compiled_rust_unit_identities,
        selected_graph_sha256, selected_graph_source_digest, selected_graph_unit_identity,
    };

    const DIRECT_COMPILER: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    /// Return a stable owner identity for the foundation sealed third-party source/artifact root.
    fn foundation_owner() -> String {
        selected_graph_sha256(b"runtime foundation owner")
    }

    /// Return a stable owner identity for compiler-owned source and target facts.
    fn toolchain_owner() -> String {
        selected_graph_sha256(b"runtime foundation toolchain owner")
    }

    /// Create one complete source-tree member for a small fixture crate.
    fn source_member(path: &str, bytes: &[u8]) -> OvenSelectedRustFacetSourceMember {
        OvenSelectedRustFacetSourceMember {
            path: path.to_string(),
            digest: selected_graph_sha256(bytes),
        }
    }

    /// Return the fixture package manifest retained with a registry source tree.
    fn fixture_cargo_toml(package: &str) -> String {
        format!("[package]\nname = \"{package}\"\nversion = \"1.0.0\"\n")
    }

    /// Return the one source member emitted for a small fixture crate.
    fn fixture_source_bytes(crate_name: &str) -> String {
        format!("pub fn {crate_name}_marker() -> u8 {{ 7 }}\n")
    }

    /// Return a source member used as the declared fixture build-script entrypoint.
    fn fixture_build_script_bytes(crate_name: &str) -> String {
        format!("fn main() {{ let _ = \"{crate_name} provider\"; }}\n")
    }

    /// Return the package member selected as the fixture build-script entrypoint.
    fn fixture_provider_package_members(crate_name: &str) -> Vec<OvenSelectedRustFacetSourceMember> {
        vec![source_member(
            "build.rs",
            fixture_build_script_bytes(crate_name).as_bytes(),
        )]
    }

    /// Build complete package-root evidence without adding it to the compiler-visible source catalogue.
    fn fixture_provider_package_source(
        unit: &OvenSelectedRustFacetUnit,
        additional_members: Vec<OvenSelectedRustFacetSourceMember>,
    ) -> Result<OvenRuntimeFoundationProviderPackageSource, Box<dyn std::error::Error>> {
        let mut members = unit
            .source_members
            .iter()
            .filter(|member| member.path != "Cargo.toml")
            .cloned()
            .collect::<Vec<_>>();
        members.extend(additional_members);
        members.sort();
        Ok(OvenRuntimeFoundationProviderPackageSource {
            root: OvenSelectedRustFacetPath {
                owner: unit.source.owner.clone(),
                path: unit.source.root.clone(),
            },
            manifest: source_member("Cargo.toml", fixture_cargo_toml(&unit.package).as_bytes()),
            members,
        })
    }

    /// Construct an explicit negative provider declaration without dropping the manifest that established it.
    fn fixture_no_build_script_declaration(
        unit: &OvenSelectedRustFacetUnit,
    ) -> Result<OvenRuntimeFoundationProviderDeclaration, Box<dyn std::error::Error>> {
        Ok(OvenRuntimeFoundationProviderDeclaration::NoBuildScript {
            package: fixture_provider_package_source(unit, Vec::new())?,
        })
    }

    /// Construct the exact fixture provider declaration without borrowing the unit's compiler-visible source list.
    fn fixture_build_script_declaration(
        unit: &OvenSelectedRustFacetUnit,
    ) -> Result<OvenRuntimeFoundationProviderDeclaration, Box<dyn std::error::Error>> {
        let package = fixture_provider_package_source(unit, fixture_provider_package_members(&unit.crate_name))?;
        let entrypoint = package
            .members
            .iter()
            .find(|member| member.path == "build.rs")
            .cloned()
            .ok_or("fixture lost declared build script")?;
        Ok(OvenRuntimeFoundationProviderDeclaration::BuildScript {
            package,
            entrypoint: entrypoint.path,
            digest: entrypoint.digest,
            edition: unit.edition.clone(),
            host_dependencies: Vec::new(),
        })
    }

    /// Construct the selected build context shared by every fixture unit.
    fn selection() -> OvenSelectedRustFacetSelection {
        OvenSelectedRustFacetSelection {
            intent: OvenSelectedRustFacetIntent {
                target: "x86_64-unknown-linux-gnu".to_string(),
                toolchain: "rustc 1.85.0 (fixture)".to_string(),
                profile: "debug".to_string(),
                features: vec!["async".to_string(), "json".to_string(), "ordinal".to_string()],
            },
            host: "aarch64-apple-darwin".to_string(),
            purpose: OvenSelectedRustFacetPurpose::Normal,
            default_features: true,
            toolchain_version: "1.85.0".to_string(),
            target_spec: OvenSelectedRustFacetTargetSpec {
                source: OvenSelectedRustFacetPath {
                    owner: toolchain_owner(),
                    path: "target-spec.json".to_string(),
                },
                digest: selected_graph_sha256(b"target spec"),
            },
        }
    }

    /// Build one source unit with a selected owner and direct graph dependencies.
    #[allow(clippy::too_many_arguments)]
    fn unit(
        selection: &OvenSelectedRustFacetSelection,
        package: &str,
        crate_name: &str,
        source_kind: OvenSelectedRustFacetSourceKind,
        owner: &str,
        role: OvenSelectedRustFacetUnitRole,
        domain: OvenSelectedRustFacetDomain,
        crate_kind: OvenSelectedRustFacetCrateKind,
        dependencies: Vec<OvenSelectedRustFacetDependency>,
    ) -> Result<OvenSelectedRustFacetUnit, Box<dyn std::error::Error>> {
        let member_bytes = fixture_source_bytes(crate_name);
        let mut members = Vec::new();
        if source_kind == OvenSelectedRustFacetSourceKind::Registry {
            let cargo_toml = fixture_cargo_toml(package);
            members.push(source_member("Cargo.toml", cargo_toml.as_bytes()));
        }
        members.push(source_member("src/lib.rs", member_bytes.as_bytes()));
        let source_identity = match source_kind {
            OvenSelectedRustFacetSourceKind::Registry => format!("registry:{package}@1.0.0"),
            OvenSelectedRustFacetSourceKind::Compiler => selected_graph_sha256(package.as_bytes()),
            OvenSelectedRustFacetSourceKind::Git
            | OvenSelectedRustFacetSourceKind::Path
            | OvenSelectedRustFacetSourceKind::Generated => {
                return Err("fixture uses only registry and compiler source kinds".into());
            }
        };
        let mut unit = OvenSelectedRustFacetUnit {
            identity: String::new(),
            package: package.to_string(),
            package_version: "1.0.0".to_string(),
            crate_name: crate_name.to_string(),
            crate_kind,
            role,
            domain,
            edition: "2024".to_string(),
            source: OvenSelectedRustFacetSource {
                kind: source_kind,
                identity: source_identity,
                owner: owner.to_string(),
                root: ".".to_string(),
                digest: selected_graph_source_digest(&members)?,
            },
            root_module: "src/lib.rs".to_string(),
            source_members: members,
            features: Vec::new(),
            default_features: false,
            cfg: Vec::new(),
            environment: BTreeMap::new(),
            include_dirs: vec![OvenSelectedRustFacetPath {
                owner: owner.to_string(),
                path: ".".to_string(),
            }],
            exclude_dirs: Vec::new(),
            dependencies,
            generated_inputs: Vec::new(),
        };
        unit.identity = selected_graph_unit_identity(selection, &unit)?;
        Ok(unit)
    }

    /// Rebind one fixture unit to its exact owner-relative source root and generated-output catalogue.
    fn bind_source_root(
        selection: &OvenSelectedRustFacetSelection,
        unit: &mut OvenSelectedRustFacetUnit,
        source_root: &str,
        generated_inputs: Vec<OvenSelectedRustFacetGeneratedInput>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        unit.source.root = source_root.to_string();
        unit.include_dirs = vec![OvenSelectedRustFacetPath {
            owner: unit.source.owner.clone(),
            path: source_root.to_string(),
        }];
        unit.generated_inputs = generated_inputs;
        unit.source.digest = selected_graph_source_digest(&unit.source_members)?;
        unit.identity = selected_graph_unit_identity(selection, unit)?;
        Ok(())
    }

    /// Construct the sealed artifact manifest used by the runtime-foundation fixture.
    fn artifacts(
        selection: &OvenSelectedRustFacetSelection,
        serde_source_digest: &str,
        serde_derive_source_digest: &str,
    ) -> OvenRustcArtifactManifest {
        let serde_source = OvenRustcRegistrySource {
            registry: "registry+https://example.invalid/index".to_string(),
            checksum: "serde-checksum".to_string(),
            relative_root: "registry-sources/serde-1.0.0".to_string(),
            digest: serde_source_digest.to_string(),
        };
        let serde_derive_source = OvenRustcRegistrySource {
            registry: "registry+https://example.invalid/index".to_string(),
            checksum: "serde-derive-checksum".to_string(),
            relative_root: "registry-sources/serde_derive-1.0.0".to_string(),
            digest: serde_derive_source_digest.to_string(),
        };
        let serde_artifact = OvenRustcArtifactExtern {
            crate_name: "serde".to_string(),
            relative_path: "deps/libserde.rlib".to_string(),
            digest: selected_graph_sha256(b"serde rlib"),
        };
        OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: OvenBuildIntent {
                target: selection.intent.target.clone(),
                toolchain: selection.intent.toolchain.clone(),
                profile: selection.intent.profile.clone(),
                features: selection.intent.features.clone(),
            },
            dependency_search_paths: vec!["deps".to_string()],
            native_search_paths: Vec::new(),
            externs: vec![
                serde_artifact.clone(),
                OvenRustcArtifactExtern {
                    crate_name: "serde_derive".to_string(),
                    relative_path: "deps/libserde_derive.dylib".to_string(),
                    digest: selected_graph_sha256(b"serde derive dylib"),
                },
            ],
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: vec![OvenRustcRegistryLeaf {
                package: "serde".to_string(),
                version: "1.0.0".to_string(),
                crate_name: "serde".to_string(),
                features: Vec::new(),
                source: serde_source.clone(),
                artifact: serde_artifact,
            }],
            registry_sources: vec![
                OvenRustcRegistrySourcePackage {
                    package: "serde".to_string(),
                    version: "1.0.0".to_string(),
                    features: Vec::new(),
                    source: serde_source,
                },
                OvenRustcRegistrySourcePackage {
                    package: "serde_derive".to_string(),
                    version: "1.0.0".to_string(),
                    features: Vec::new(),
                    source: serde_derive_source,
                },
            ],
            compile_environment: BTreeMap::new(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: vec![
                OvenRustcSupportingArtifact {
                    relative_path: "registry-sources/serde-1.0.0/Cargo.toml".to_string(),
                    digest: selected_graph_sha256(fixture_cargo_toml("serde").as_bytes()),
                },
                OvenRustcSupportingArtifact {
                    relative_path: "registry-sources/serde_derive-1.0.0/Cargo.toml".to_string(),
                    digest: selected_graph_sha256(fixture_cargo_toml("serde_derive").as_bytes()),
                },
                OvenRustcSupportingArtifact {
                    relative_path: "generated/serde/private.rs".to_string(),
                    digest: selected_graph_sha256(b"serde private.rs"),
                },
            ],
        }
    }

    /// Build a complete mixed-domain runtime foundation with two prebuilt leaves and two compiler-owned rebuilds.
    fn foundation() -> Result<OvenRuntimeFoundation, Box<dyn std::error::Error>> {
        let selection = selection();
        let mut serde = unit(
            &selection,
            "serde",
            "serde",
            OvenSelectedRustFacetSourceKind::Registry,
            &foundation_owner(),
            OvenSelectedRustFacetUnitRole::Library,
            OvenSelectedRustFacetDomain::Target,
            OvenSelectedRustFacetCrateKind::Rlib,
            Vec::new(),
        )?;
        bind_source_root(
            &selection,
            &mut serde,
            "registry-sources/serde-1.0.0",
            vec![OvenSelectedRustFacetGeneratedInput {
                name: "serde-private".to_string(),
                source: OvenSelectedRustFacetPath {
                    owner: foundation_owner(),
                    path: "generated/serde/private.rs".to_string(),
                },
                digest: selected_graph_sha256(b"serde private.rs"),
            }],
        )?;
        let mut serde_derive = unit(
            &selection,
            "serde_derive",
            "serde_derive",
            OvenSelectedRustFacetSourceKind::Registry,
            &foundation_owner(),
            OvenSelectedRustFacetUnitRole::ProcMacro,
            OvenSelectedRustFacetDomain::Host,
            OvenSelectedRustFacetCrateKind::ProcMacro,
            Vec::new(),
        )?;
        serde_derive.cfg = vec!["provider_cfg".to_string()];
        bind_source_root(
            &selection,
            &mut serde_derive,
            "registry-sources/serde_derive-1.0.0",
            Vec::new(),
        )?;
        let mut core = unit(
            &selection,
            "incan_core",
            "incan_core",
            OvenSelectedRustFacetSourceKind::Compiler,
            &toolchain_owner(),
            OvenSelectedRustFacetUnitRole::Library,
            OvenSelectedRustFacetDomain::Target,
            OvenSelectedRustFacetCrateKind::Rlib,
            vec![
                OvenSelectedRustFacetDependency {
                    alias: "serde".to_string(),
                    unit: serde.identity.clone(),
                },
                OvenSelectedRustFacetDependency {
                    alias: "serde_derive".to_string(),
                    unit: serde_derive.identity.clone(),
                },
            ],
        )?;
        bind_source_root(&selection, &mut core, "compiler/incan_core", Vec::new())?;
        let mut stdlib = unit(
            &selection,
            "incan_stdlib",
            "incan_stdlib",
            OvenSelectedRustFacetSourceKind::Compiler,
            &toolchain_owner(),
            OvenSelectedRustFacetUnitRole::Library,
            OvenSelectedRustFacetDomain::Target,
            OvenSelectedRustFacetCrateKind::Rlib,
            vec![OvenSelectedRustFacetDependency {
                alias: "incan_core".to_string(),
                unit: core.identity.clone(),
            }],
        )?;
        bind_source_root(&selection, &mut stdlib, "compiler/incan_stdlib", Vec::new())?;
        let artifacts = artifacts(&selection, &serde.source.digest, &serde_derive.source.digest);
        let units = vec![
            OvenRuntimeFoundationUnit {
                selected_identity: serde.identity.clone(),
                domain: serde.domain,
                execution: OvenRuntimeFoundationUnitExecution::Prebuilt {
                    artifact: artifacts.externs[0].clone(),
                },
            },
            OvenRuntimeFoundationUnit {
                selected_identity: serde_derive.identity.clone(),
                domain: serde_derive.domain,
                execution: OvenRuntimeFoundationUnitExecution::Prebuilt {
                    artifact: artifacts.externs[1].clone(),
                },
            },
            OvenRuntimeFoundationUnit {
                selected_identity: core.identity.clone(),
                domain: core.domain,
                execution: OvenRuntimeFoundationUnitExecution::Rebuild,
            },
            OvenRuntimeFoundationUnit {
                selected_identity: stdlib.identity.clone(),
                domain: stdlib.domain,
                execution: OvenRuntimeFoundationUnitExecution::Rebuild,
            },
        ];
        Ok(OvenRuntimeFoundation {
            schema_version: OVEN_RUNTIME_FOUNDATION_SCHEMA_VERSION,
            compiler_closure_digest: DIRECT_COMPILER.to_string(),
            artifact_owner: foundation_owner(),
            artifacts,
            selected_graph: OvenSelectedRustFacetGraph {
                schema_version: OVEN_SELECTED_RUST_FACET_GRAPH_SCHEMA_VERSION,
                selection,
                owners: vec![
                    OvenSelectedRustFacetOwner {
                        identity: foundation_owner(),
                        kind: OvenSelectedRustFacetOwnerKind::Constituent,
                    },
                    OvenSelectedRustFacetOwner {
                        identity: toolchain_owner(),
                        kind: OvenSelectedRustFacetOwnerKind::Toolchain,
                    },
                ],
                units: vec![serde, serde_derive, core, stdlib.clone()],
                exposed_roots: BTreeMap::from([("incan_stdlib".to_string(), stdlib.identity)]),
            },
            units,
        })
    }

    /// Build an exhaustive fixture provider catalogue with generated-output and cfg-only build-script evidence.
    fn provider_records(
        foundation: &OvenRuntimeFoundation,
    ) -> Result<Vec<OvenRuntimeFoundationProviderRecord>, Box<dyn std::error::Error>> {
        foundation
            .selected_graph
            .units
            .iter()
            .map(|unit| {
                let declaration = match unit.crate_name.as_str() {
                    "serde" | "serde_derive" => fixture_build_script_declaration(unit)?,
                    _ => fixture_no_build_script_declaration(unit)?,
                };
                let effects = |emitted_cfg: Vec<String>, generated_inputs: Vec<OvenSelectedRustFacetGeneratedInput>| {
                    OvenRuntimeFoundationProviderEffects {
                        generated_inputs,
                        emitted_cfg,
                        checked_cfg: vec!["cfg(provider_cfg)".to_string()],
                        emitted_environment: BTreeMap::new(),
                        rerun_paths: vec!["build.rs".to_string()],
                        rerun_environment: Vec::new(),
                        native_link: OvenRuntimeFoundationNativeLinkState::NoNativeLink,
                    }
                };
                let state = match unit.crate_name.as_str() {
                    "serde" => OvenRuntimeFoundationProviderState::Captured {
                        receipt: OvenRuntimeFoundationProviderReceipt {
                            schema_version: OVEN_RUNTIME_FOUNDATION_PROVIDER_RECEIPT_SCHEMA_VERSION,
                            provider_receipt_identity: selected_graph_sha256(b"fixture provider receipt"),
                            effect_digest: selected_graph_sha256(b"fixture provider effects"),
                            effects: effects(Vec::new(), unit.generated_inputs.clone()),
                        },
                    },
                    "serde_derive" => OvenRuntimeFoundationProviderState::Captured {
                        receipt: OvenRuntimeFoundationProviderReceipt {
                            schema_version: OVEN_RUNTIME_FOUNDATION_PROVIDER_RECEIPT_SCHEMA_VERSION,
                            provider_receipt_identity: selected_graph_sha256(b"fixture provider receipt"),
                            effect_digest: selected_graph_sha256(b"fixture provider effects"),
                            effects: effects(vec!["provider_cfg".to_string()], Vec::new()),
                        },
                    },
                    _ => OvenRuntimeFoundationProviderState::NoProvider,
                };
                Ok(OvenRuntimeFoundationProviderRecord {
                    selected_identity: unit.identity.clone(),
                    declaration,
                    state,
                })
            })
            .collect()
    }

    /// Construct one canonical release asset around the fixture foundation.
    fn foundation_asset() -> Result<OvenRuntimeFoundationAsset, Box<dyn std::error::Error>> {
        let foundation = foundation()?;
        OvenRuntimeFoundationAsset::sealed(foundation.clone(), provider_records(&foundation)?).map_err(Into::into)
    }

    /// Build one Incan provider-intake response for the fixture's compiler-owned `incan_core` unit.
    fn provider_intake_build_script_response(
        selected_identity: &str,
        host_alias: &str,
        host_unit: &str,
        package_members: &[OvenSelectedRustFacetSourceMember],
    ) -> serde_json::Value {
        let entrypoint = package_members
            .iter()
            .find(|member| member.path == "build.rs")
            .expect("fixture intake response lost build.rs");
        let mut response = serde_json::json!({
            "schema": OVEN_RUNTIME_FOUNDATION_PROVIDER_INTAKE_SCHEMA,
            "selected_identity": selected_identity,
            "status": "build_script",
            "declaration": {
                "entrypoint": { "path": entrypoint.path, "digest": entrypoint.digest },
                "package_members": [],
                "host_dependencies": [
                    { "alias": host_alias, "unit": host_unit }
                ]
            }
        });
        response["declaration"]["package_members"] = serde_json::Value::Array(
            package_members
                .iter()
                .map(|member| serde_json::json!({ "path": member.path, "digest": member.digest }))
                .collect(),
        );
        response
    }

    /// Bind a source-wire fixture to the retained manifest, package inventory and selected host edge it requested.
    fn provider_intake_evidence(
        selected: &ValidatedOvenSelectedRustFacetGraph,
        unit: &OvenSelectedRustFacetUnit,
        host_dependencies: Vec<OvenRuntimeFoundationProviderIntakeHostDependency>,
    ) -> Result<OvenRuntimeFoundationProviderIntakeEvidence, Box<dyn std::error::Error>> {
        OvenRuntimeFoundationProviderIntakeEvidence::new(
            selected,
            unit.identity.clone(),
            fixture_provider_package_source(unit, fixture_provider_package_members(&unit.crate_name))?,
            host_dependencies,
        )
        .map_err(Into::into)
    }

    /// Write one exact foundation fixture member below a temporary retained owner root.
    fn write_fixture_file(root: &Path, relative: &str, bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
        let path = root.join(relative);
        let parent = path.parent().ok_or("fixture path has no parent")?;
        fs::create_dir_all(parent)?;
        fs::write(path, bytes)?;
        Ok(())
    }

    /// Populate the exact two-root physical payload named by the runtime-foundation fixture.
    fn write_materialization_fixture(
        foundation_root: &Path,
        toolchain_root: &Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
        write_fixture_file(
            foundation_root,
            "registry-sources/serde-1.0.0/Cargo.toml",
            fixture_cargo_toml("serde").as_bytes(),
        )?;
        write_fixture_file(
            foundation_root,
            "registry-sources/serde-1.0.0/src/lib.rs",
            fixture_source_bytes("serde").as_bytes(),
        )?;
        write_fixture_file(
            foundation_root,
            "registry-sources/serde-1.0.0/build.rs",
            fixture_build_script_bytes("serde").as_bytes(),
        )?;
        write_fixture_file(
            foundation_root,
            "registry-sources/serde_derive-1.0.0/Cargo.toml",
            fixture_cargo_toml("serde_derive").as_bytes(),
        )?;
        write_fixture_file(
            foundation_root,
            "registry-sources/serde_derive-1.0.0/src/lib.rs",
            fixture_source_bytes("serde_derive").as_bytes(),
        )?;
        write_fixture_file(
            foundation_root,
            "registry-sources/serde_derive-1.0.0/build.rs",
            fixture_build_script_bytes("serde_derive").as_bytes(),
        )?;
        write_fixture_file(foundation_root, "generated/serde/private.rs", b"serde private.rs")?;
        write_fixture_file(foundation_root, "deps/libserde.rlib", b"serde rlib")?;
        write_fixture_file(foundation_root, "deps/libserde_derive.dylib", b"serde derive dylib")?;
        write_fixture_file(toolchain_root, "target-spec.json", b"target spec")?;
        write_fixture_file(
            toolchain_root,
            "compiler/incan_core/Cargo.toml",
            fixture_cargo_toml("incan_core").as_bytes(),
        )?;
        write_fixture_file(
            toolchain_root,
            "compiler/incan_core/src/lib.rs",
            fixture_source_bytes("incan_core").as_bytes(),
        )?;
        write_fixture_file(
            toolchain_root,
            "compiler/incan_stdlib/Cargo.toml",
            fixture_cargo_toml("incan_stdlib").as_bytes(),
        )?;
        write_fixture_file(
            toolchain_root,
            "compiler/incan_stdlib/src/lib.rs",
            fixture_source_bytes("incan_stdlib").as_bytes(),
        )?;
        Ok(())
    }

    /// Populate only the compiler-visible portion of the fixture for lower-level foundation materialization tests.
    ///
    /// A bare foundation deliberately has no provider records, so it must not admit provider-only files by itself.
    fn write_foundation_materialization_fixture(
        foundation_root: &Path,
        toolchain_root: &Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
        write_materialization_fixture(foundation_root, toolchain_root)?;
        fs::remove_file(foundation_root.join("registry-sources/serde-1.0.0/build.rs"))?;
        fs::remove_file(foundation_root.join("registry-sources/serde_derive-1.0.0/build.rs"))?;
        fs::remove_file(toolchain_root.join("compiler/incan_core/Cargo.toml"))?;
        fs::remove_file(toolchain_root.join("compiler/incan_stdlib/Cargo.toml"))?;
        Ok(())
    }

    /// Write the one canonical foundation descriptor at the root of a complete fixture asset.
    fn write_foundation_descriptor(
        foundation_root: &Path,
        asset: &OvenRuntimeFoundationAsset,
    ) -> Result<(), Box<dyn std::error::Error>> {
        fs::write(
            foundation_root.join(OVEN_RUNTIME_FOUNDATION_ASSET_FILENAME),
            serde_json::to_vec(asset)?,
        )?;
        Ok(())
    }

    /// Bind the fixture's two immutable roots to its selected owner table.
    fn materialization_owner_roots(
        foundation_root: &Path,
        toolchain_root: &Path,
    ) -> Vec<OvenSelectedRustFacetOwnerRoot> {
        vec![
            OvenSelectedRustFacetOwnerRoot {
                identity: foundation_owner(),
                root: foundation_root.to_path_buf(),
            },
            OvenSelectedRustFacetOwnerRoot {
                identity: toolchain_owner(),
                root: toolchain_root.to_path_buf(),
            },
        ]
    }

    /// A foundation retains the explicit host proc macro alongside target rlibs and projects only direct prebuilt
    /// graph edges for the compiler-owned unit that consumes them.
    #[test]
    fn runtime_foundation_preserves_host_target_and_rebuild_boundary() -> Result<(), Box<dyn std::error::Error>> {
        let foundation = foundation()?.validated()?;
        let core = foundation
            .selected_graph()
            .graph()
            .units
            .iter()
            .find(|unit| unit.crate_name == "incan_core")
            .ok_or("fixture lost incan_core")?;
        let dependencies = foundation.prebuilt_dependencies(&core.identity)?;
        assert_eq!(foundation.compiler_closure_digest(), DIRECT_COMPILER);
        assert_eq!(foundation.rebuild_units().count(), 2);
        assert_eq!(dependencies.len(), 2);
        assert!(dependencies.iter().any(|dependency| {
            dependency.alias == "serde" && dependency.domain == OvenSelectedRustFacetDomain::Target
        }));
        assert!(dependencies.iter().any(|dependency| {
            dependency.alias == "serde_derive" && dependency.domain == OvenSelectedRustFacetDomain::Host
        }));
        Ok(())
    }

    /// A prebuilt unit cannot quietly change from target to host simply because its artifact filename is reusable.
    #[test]
    fn runtime_foundation_refuses_domain_mismatch() -> Result<(), Box<dyn std::error::Error>> {
        let mut foundation = foundation()?;
        let policy = foundation
            .units
            .iter_mut()
            .find(|policy| matches!(policy.execution, OvenRuntimeFoundationUnitExecution::Prebuilt { .. }))
            .ok_or("fixture lost prebuilt policy")?;
        policy.domain = OvenSelectedRustFacetDomain::Host;
        assert!(matches!(
            foundation.validated(),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation unit domain",
                ..
            })
        ));
        Ok(())
    }

    /// Registry source cannot become a compiler-owned rebuild just because source bytes happen to be present.
    #[test]
    fn runtime_foundation_refuses_registry_rebuild() -> Result<(), Box<dyn std::error::Error>> {
        let mut foundation = foundation()?;
        let policy = foundation
            .units
            .iter_mut()
            .find(|policy| matches!(policy.execution, OvenRuntimeFoundationUnitExecution::Prebuilt { .. }))
            .ok_or("fixture lost prebuilt policy")?;
        policy.execution = OvenRuntimeFoundationUnitExecution::Rebuild;
        assert!(matches!(
            foundation.validated(),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation rebuild unit",
                ..
            })
        ));
        Ok(())
    }

    /// A declared prebuilt artifact must be present byte-for-byte in the sealed artifact manifest.
    #[test]
    fn runtime_foundation_refuses_undeclared_prebuilt_artifact() -> Result<(), Box<dyn std::error::Error>> {
        let mut foundation = foundation()?;
        let policy = foundation
            .units
            .iter_mut()
            .find(|policy| matches!(policy.execution, OvenRuntimeFoundationUnitExecution::Prebuilt { .. }))
            .ok_or("fixture lost prebuilt policy")?;
        let OvenRuntimeFoundationUnitExecution::Prebuilt { artifact } = &mut policy.execution else {
            return Err("fixture prebuilt policy changed modes".into());
        };
        artifact.digest = selected_graph_sha256(b"substituted artifact");
        assert!(matches!(
            foundation.validated(),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation prebuilt artifact",
                ..
            })
        ));
        Ok(())
    }

    /// Build-script output used by a prebuilt crate must remain a digest-verified member of the sealed foundation.
    #[test]
    fn runtime_foundation_refuses_unsealed_generated_input() -> Result<(), Box<dyn std::error::Error>> {
        let mut foundation = foundation()?;
        foundation
            .artifacts
            .supporting_artifacts
            .retain(|artifact| artifact.relative_path != "generated/serde/private.rs");
        assert!(matches!(
            foundation.validated(),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation generated input",
                ..
            })
        ));
        Ok(())
    }

    /// Every selected unit requires an explicit rebuild or prebuilt policy, so omission cannot become a fallback.
    #[test]
    fn runtime_foundation_refuses_missing_unit_policy() -> Result<(), Box<dyn std::error::Error>> {
        let mut foundation = foundation()?;
        let _ = foundation.units.pop().ok_or("fixture has no policies")?;
        assert!(matches!(
            foundation.validated(),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation units",
                ..
            })
        ));
        Ok(())
    }

    /// A release asset has one canonical identity and an explicit provider state for every selected unit.
    #[test]
    fn runtime_foundation_asset_binds_complete_provider_state() -> Result<(), Box<dyn std::error::Error>> {
        let asset = foundation_asset()?.validated()?;
        assert!(asset.foundation_identity().starts_with("sha256:"));
        let serde = asset
            .foundation()
            .selected_graph()
            .graph()
            .units
            .iter()
            .find(|unit| unit.crate_name == "serde")
            .ok_or("fixture lost serde")?;
        assert!(matches!(
            asset.provider_state(&serde.identity),
            Some(OvenRuntimeFoundationProviderState::Captured { receipt })
                if receipt.effects.generated_inputs == serde.generated_inputs
                    && matches!(receipt.effects.native_link, OvenRuntimeFoundationNativeLinkState::NoNativeLink)
        ));
        assert!(matches!(
            asset.provider_record(&serde.identity),
            Some(OvenRuntimeFoundationProviderRecord {
                declaration: OvenRuntimeFoundationProviderDeclaration::BuildScript {
                    package,
                    entrypoint,
                    ..
                },
                ..
            }) if entrypoint == "build.rs"
                && package.members.iter().any(|member| member.path == "build.rs")
        ));
        assert!(
            !serde.source_members.iter().any(|member| member.path == "build.rs"),
            "provider-only build source must stay out of the compiler-visible unit catalogue"
        );
        let serde_derive = asset
            .foundation()
            .selected_graph()
            .graph()
            .units
            .iter()
            .find(|unit| unit.crate_name == "serde_derive")
            .ok_or("fixture lost serde_derive")?;
        assert!(matches!(
            asset.provider_state(&serde_derive.identity),
            Some(OvenRuntimeFoundationProviderState::Captured { receipt })
                if receipt.effects.generated_inputs.is_empty()
                    && receipt.effects.emitted_cfg == ["provider_cfg"]
        ));
        Ok(())
    }

    /// Package-root evidence changes the release-asset payload, so an earlier asset schema cannot be admitted under
    /// the new interpretation by merely carrying otherwise well-formed fields.
    #[test]
    fn runtime_foundation_asset_refuses_previous_schema() -> Result<(), Box<dyn std::error::Error>> {
        let mut asset = foundation_asset()?;
        asset.schema_version = OVEN_RUNTIME_FOUNDATION_ASSET_SCHEMA_VERSION - 1;
        assert!(matches!(
            asset.validated(),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation asset schema",
                ..
            })
        ));
        Ok(())
    }

    /// A source-validated build-dependency declaration must still name the exact host edge already selected for its
    /// owning unit; a host unit elsewhere in the graph is not an interchangeable provider compiler input.
    #[test]
    fn runtime_foundation_asset_requires_provider_host_dependencies_to_match_selected_edges()
    -> Result<(), Box<dyn std::error::Error>> {
        let asset = foundation_asset()?;
        let core = asset
            .foundation
            .selected_graph
            .units
            .iter()
            .find(|unit| unit.crate_name == "incan_core")
            .cloned()
            .ok_or("fixture lost incan_core")?;
        let serde_derive_identity = asset
            .foundation
            .selected_graph
            .units
            .iter()
            .find(|unit| unit.crate_name == "serde_derive")
            .map(|unit| unit.identity.clone())
            .ok_or("fixture lost serde_derive")?;

        let mut valid_providers = asset.providers.clone();
        let valid = valid_providers
            .iter_mut()
            .find(|record| record.selected_identity == core.identity)
            .ok_or("fixture lost incan_core provider record")?;
        let mut declaration = fixture_build_script_declaration(&core)?;
        let OvenRuntimeFoundationProviderDeclaration::BuildScript { host_dependencies, .. } = &mut declaration else {
            return Err("fixture build-script declaration changed shape".into());
        };
        *host_dependencies = vec![OvenRuntimeFoundationProviderHostDependency {
            alias: "serde_derive".to_string(),
            unit: serde_derive_identity.clone(),
        }];
        valid.declaration = declaration;
        valid.state = OvenRuntimeFoundationProviderState::Unsupported {
            reason: "fixture records the direct host edge without executing it".to_string(),
        };
        OvenRuntimeFoundationAsset::sealed(asset.foundation.clone(), valid_providers)?;

        let mut detached_providers = asset.providers;
        let detached = detached_providers
            .iter_mut()
            .find(|record| record.selected_identity == core.identity)
            .ok_or("fixture lost incan_core provider record")?;
        let mut declaration = fixture_build_script_declaration(&core)?;
        let OvenRuntimeFoundationProviderDeclaration::BuildScript { host_dependencies, .. } = &mut declaration else {
            return Err("fixture build-script declaration changed shape".into());
        };
        *host_dependencies = vec![OvenRuntimeFoundationProviderHostDependency {
            alias: "not_a_selected_edge".to_string(),
            unit: serde_derive_identity,
        }];
        detached.declaration = declaration;
        detached.state = OvenRuntimeFoundationProviderState::Unsupported {
            reason: "fixture must refuse a host unit detached from its selected edge".to_string(),
        };
        assert!(matches!(
            OvenRuntimeFoundationAsset::sealed(asset.foundation, detached_providers),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation provider host dependencies",
                ..
            })
        ));
        Ok(())
    }

    /// A package inventory is relative to the package root, so it must retain selected compiler members beneath a
    /// nested compiler root rather than silently describing only the manifest and build script.
    #[test]
    fn runtime_foundation_provider_package_inventory_covers_nested_compiler_root()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut unit = foundation()?
            .selected_graph
            .units
            .into_iter()
            .find(|unit| unit.crate_name == "incan_core")
            .ok_or("fixture lost incan_core")?;
        unit.source.root = "compiler/incan_core/src".to_string();
        unit.root_module = "lib.rs".to_string();
        unit.source_members = vec![source_member("lib.rs", fixture_source_bytes("incan_core").as_bytes())];
        unit.source.digest = selected_graph_source_digest(&unit.source_members)?;
        let package = OvenRuntimeFoundationProviderPackageSource {
            root: OvenSelectedRustFacetPath {
                owner: unit.source.owner.clone(),
                path: "compiler/incan_core".to_string(),
            },
            manifest: source_member("Cargo.toml", fixture_cargo_toml("incan_core").as_bytes()),
            members: vec![
                source_member("build.rs", fixture_build_script_bytes("incan_core").as_bytes()),
                source_member("src/lib.rs", fixture_source_bytes("incan_core").as_bytes()),
            ],
        };
        validate_runtime_foundation_provider_package_source(&unit, &package)?;

        let mut incomplete = package.clone();
        incomplete.members.retain(|member| member.path != "src/lib.rs");
        assert!(matches!(
            validate_runtime_foundation_provider_package_source(&unit, &incomplete),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation provider source members",
                ..
            })
        ));
        Ok(())
    }

    /// The Rust host accepts only the Incan source declaration that matches the selected unit's identity and direct
    /// host edge; it derives edition and receipt-adjacent declaration facts from the selected graph rather than wire.
    #[test]
    fn runtime_foundation_provider_intake_projects_authenticated_source_declaration()
    -> Result<(), Box<dyn std::error::Error>> {
        let selected = foundation()?.selected_graph.validated()?;
        let core = selected
            .graph()
            .units
            .iter()
            .find(|unit| unit.crate_name == "incan_core")
            .ok_or("fixture lost incan_core")?;
        let serde_derive = selected
            .graph()
            .units
            .iter()
            .find(|unit| unit.crate_name == "serde_derive")
            .ok_or("fixture lost serde_derive")?;
        let expected_host_dependencies = vec![OvenRuntimeFoundationProviderHostDependency {
            alias: "serde_derive".to_string(),
            unit: serde_derive.identity.clone(),
        }];
        let evidence = provider_intake_evidence(
            &selected,
            core,
            vec![OvenRuntimeFoundationProviderIntakeHostDependency {
                declaration_index: 17,
                alias: "serde_derive".to_string(),
                unit: serde_derive.identity.clone(),
            }],
        )?;
        let response = serde_json::to_vec(&provider_intake_build_script_response(
            &core.identity,
            "serde_derive",
            &serde_derive.identity,
            &evidence.package.members,
        ))?;
        let declaration = decode_runtime_foundation_provider_intake(&selected, &evidence, &response)?;
        assert!(matches!(
            declaration,
            OvenRuntimeFoundationProviderDeclaration::BuildScript {
                ref package,
                ref entrypoint,
                ref digest,
                ref edition,
                ref host_dependencies,
            } if package == &evidence.package
                && entrypoint == "build.rs"
                && digest == &selected_graph_sha256(fixture_build_script_bytes("incan_core").as_bytes())
                && edition == &core.edition
                && host_dependencies == &expected_host_dependencies
        ));

        let no_build_script = serde_json::to_vec(&serde_json::json!({
            "schema": OVEN_RUNTIME_FOUNDATION_PROVIDER_INTAKE_SCHEMA,
            "selected_identity": core.identity,
            "status": "no_build_script",
        }))?;
        assert!(matches!(
            decode_runtime_foundation_provider_intake(&selected, &evidence, &no_build_script),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation provider intake",
                ..
            })
        ));
        let no_grant_evidence = provider_intake_evidence(&selected, core, Vec::new())?;
        assert!(matches!(
            decode_runtime_foundation_provider_intake(&selected, &no_grant_evidence, &no_build_script)?,
            OvenRuntimeFoundationProviderDeclaration::NoBuildScript { package } if package == no_grant_evidence.package
        ));

        let legacy = serde_json::to_vec(&serde_json::json!({
            "schema": "incan.oven.runtime-foundation-provider-intake/1",
            "selected_identity": core.identity,
            "status": "no_build_script",
        }))?;
        assert!(matches!(
            decode_runtime_foundation_provider_intake(&selected, &evidence, &legacy),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation provider intake",
                ..
            })
        ));
        Ok(())
    }

    /// Host request construction retains the manifest declaration slot correlation long enough for the Incan source
    /// bridge to validate it, then refuses any manifest text that does not match the typed package evidence.
    #[test]
    fn runtime_foundation_provider_intake_encodes_authenticated_slot_bearing_request()
    -> Result<(), Box<dyn std::error::Error>> {
        let selected = foundation()?.selected_graph.validated()?;
        let core = selected
            .graph()
            .units
            .iter()
            .find(|unit| unit.crate_name == "incan_core")
            .ok_or("fixture lost incan_core")?;
        let serde_derive = selected
            .graph()
            .units
            .iter()
            .find(|unit| unit.crate_name == "serde_derive")
            .ok_or("fixture lost serde_derive")?;
        let manifest_source = format!(
            "[package]\nname = \"{}\"\nversion = \"1.0.0\"\nbuild = \"build.rs\"\n",
            core.package
        );
        let mut package = fixture_provider_package_source(core, fixture_provider_package_members(&core.crate_name))?;
        package.manifest = source_member("Cargo.toml", manifest_source.as_bytes());
        let evidence = OvenRuntimeFoundationProviderIntakeEvidence::new(
            &selected,
            core.identity.clone(),
            package,
            vec![OvenRuntimeFoundationProviderIntakeHostDependency {
                declaration_index: 17,
                alias: "serde_derive".to_string(),
                unit: serde_derive.identity.clone(),
            }],
        )?;

        let request = encode_runtime_foundation_provider_intake_request(&selected, &evidence, &manifest_source)?;
        let request = serde_json::from_slice::<serde_json::Value>(&request)?;
        assert_eq!(
            request["schema"],
            serde_json::Value::String(OVEN_RUNTIME_FOUNDATION_PROVIDER_INTAKE_REQUEST_SCHEMA.to_string())
        );
        assert_eq!(request["request"]["selected_identity"], core.identity);
        assert_eq!(request["request"]["manifest"]["path"], "Cargo.toml");
        assert_eq!(request["request"]["manifest_source"], manifest_source);
        assert_eq!(
            request["request"]["package_members"].as_array().map(Vec::len),
            Some(evidence.package.members.len())
        );
        assert_eq!(
            request["request"]["host_dependencies"],
            serde_json::json!([{
                "declaration_index": 17,
                "alias": "serde_derive",
                "unit": serde_derive.identity,
            }])
        );
        for forbidden in ["root", "edition", "effects", "generated_inputs"] {
            assert!(
                request["request"].get(forbidden).is_none(),
                "source request must not carry {forbidden}"
            );
        }

        assert!(matches!(
            encode_runtime_foundation_provider_intake_request(
                &selected,
                &evidence,
                &(manifest_source.clone() + "# substituted")
            ),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation provider intake manifest",
                ..
            })
        ));

        let invalid_package =
            fixture_provider_package_source(core, fixture_provider_package_members(&core.crate_name))?;
        assert!(matches!(
            OvenRuntimeFoundationProviderIntakeEvidence::new(
                &selected,
                core.identity.clone(),
                invalid_package,
                vec![OvenRuntimeFoundationProviderIntakeHostDependency {
                    declaration_index: 3,
                    alias: "not_a_selected_edge".to_string(),
                    unit: serde_derive.identity.clone(),
                }],
            ),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation provider host dependencies",
                ..
            })
        ));
        Ok(())
    }

    /// Source wire text cannot grant an unrelated host unit, an execution effect, or a no-provider fallback when the
    /// checked Incan declaration itself refused.
    #[test]
    fn runtime_foundation_provider_intake_refuses_detached_or_effectful_source_wire()
    -> Result<(), Box<dyn std::error::Error>> {
        let selected = foundation()?.selected_graph.validated()?;
        let core = selected
            .graph()
            .units
            .iter()
            .find(|unit| unit.crate_name == "incan_core")
            .ok_or("fixture lost incan_core")?;
        let serde_derive = selected
            .graph()
            .units
            .iter()
            .find(|unit| unit.crate_name == "serde_derive")
            .ok_or("fixture lost serde_derive")?;
        let evidence = provider_intake_evidence(
            &selected,
            core,
            vec![OvenRuntimeFoundationProviderIntakeHostDependency {
                declaration_index: 3,
                alias: "serde_derive".to_string(),
                unit: serde_derive.identity.clone(),
            }],
        )?;

        let detached = serde_json::to_vec(&provider_intake_build_script_response(
            &core.identity,
            "not_a_selected_edge",
            &serde_derive.identity,
            &evidence.package.members,
        ))?;
        assert!(matches!(
            decode_runtime_foundation_provider_intake(&selected, &evidence, &detached),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation provider intake",
                ..
            })
        ));

        let incomplete_members = evidence
            .package
            .members
            .iter()
            .filter(|member| member.path != "src/lib.rs")
            .cloned()
            .collect::<Vec<_>>();
        let incomplete = serde_json::to_vec(&provider_intake_build_script_response(
            &core.identity,
            "serde_derive",
            &serde_derive.identity,
            &incomplete_members,
        ))?;
        assert!(matches!(
            decode_runtime_foundation_provider_intake(&selected, &evidence, &incomplete),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation provider intake",
                ..
            })
        ));

        let mut effectful = provider_intake_build_script_response(
            &core.identity,
            "serde_derive",
            &serde_derive.identity,
            &evidence.package.members,
        );
        effectful["declaration"]["effects"] = serde_json::json!({ "generated_inputs": [] });
        let effectful = serde_json::to_vec(&effectful)?;
        assert!(matches!(
            decode_runtime_foundation_provider_intake(&selected, &evidence, &effectful),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation provider intake",
                ..
            })
        ));

        let refused = serde_json::to_vec(&serde_json::json!({
            "schema": OVEN_RUNTIME_FOUNDATION_PROVIDER_INTAKE_SCHEMA,
            "selected_identity": core.identity,
            "status": "refused",
            "error": { "kind": "missing", "fields": ["package", "build"] },
        }))?;
        assert!(matches!(
            decode_runtime_foundation_provider_intake(&selected, &evidence, &refused),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation provider intake",
                ..
            })
        ));
        Ok(())
    }

    /// Provider source bytes replace the release-asset authority and receipt, but not an unchanged Rust unit's
    /// compiler-visible JEC identity.
    #[test]
    fn runtime_foundation_asset_separates_provider_source_from_compiled_unit_identity()
    -> Result<(), Box<dyn std::error::Error>> {
        let asset = foundation_asset()?;
        let selected = asset.foundation.selected_graph.clone().validated()?;
        let before = compiled_rust_unit_identities(&selected, DIRECT_COMPILER)?;
        let original_receipt = asset
            .providers
            .iter()
            .find_map(|record| match &record.state {
                OvenRuntimeFoundationProviderState::Captured { receipt } => {
                    Some(receipt.provider_receipt_identity.clone())
                }
                OvenRuntimeFoundationProviderState::NoProvider
                | OvenRuntimeFoundationProviderState::Unsupported { .. } => None,
            })
            .ok_or("fixture lost captured provider receipt")?;
        let mut changed = asset.clone();
        let record = changed
            .providers
            .iter_mut()
            .find(|record| {
                matches!(
                    record.declaration,
                    OvenRuntimeFoundationProviderDeclaration::BuildScript { .. }
                )
            })
            .ok_or("fixture lost build-script provider record")?;
        let OvenRuntimeFoundationProviderDeclaration::BuildScript { package, digest, .. } = &mut record.declaration
        else {
            return Err("fixture provider declaration changed shape".into());
        };
        let member = package
            .members
            .iter_mut()
            .find(|member| member.path == "build.rs")
            .ok_or("fixture lost provider build source")?;
        member.digest = selected_graph_sha256(b"fn main() { println!(\"changed\"); }\n");
        *digest = member.digest.clone();
        let changed = OvenRuntimeFoundationAsset::sealed(changed.foundation, changed.providers)?;
        let after = compiled_rust_unit_identities(&selected, DIRECT_COMPILER)?;
        let changed_receipt = changed
            .providers
            .iter()
            .find_map(|record| match &record.state {
                OvenRuntimeFoundationProviderState::Captured { receipt } => {
                    Some(receipt.provider_receipt_identity.as_str())
                }
                OvenRuntimeFoundationProviderState::NoProvider
                | OvenRuntimeFoundationProviderState::Unsupported { .. } => None,
            })
            .ok_or("changed fixture lost captured provider receipt")?;

        assert_ne!(changed.foundation_identity, asset.foundation_identity);
        assert_ne!(changed_receipt, original_receipt);
        assert_eq!(after, before);
        Ok(())
    }

    /// A provider that ran without native-link output must say so; an absent field is not an empty plan.
    #[test]
    fn runtime_foundation_asset_requires_explicit_native_link_outcome() -> Result<(), Box<dyn std::error::Error>> {
        let asset = foundation_asset()?;
        let mut wire = serde_json::to_value(asset)?;
        let captured = wire["providers"]
            .as_array_mut()
            .ok_or("runtime-foundation asset lost provider array")?
            .iter_mut()
            .find(|record| record["state"].get("state").and_then(serde_json::Value::as_str) == Some("captured"))
            .ok_or("fixture lost captured provider state")?;
        captured["state"]["receipt"]["effects"]
            .as_object_mut()
            .ok_or("captured provider effects were not an object")?
            .remove("native_link");
        assert!(serde_json::from_value::<OvenRuntimeFoundationAsset>(wire).is_err());
        Ok(())
    }

    /// A captured provider receipt is a complete typed record; a missing retained directive cannot decode as empty.
    #[test]
    fn runtime_foundation_asset_requires_complete_typed_provider_effects() -> Result<(), Box<dyn std::error::Error>> {
        let asset = foundation_asset()?;
        let mut wire = serde_json::to_value(asset)?;
        let captured = wire["providers"]
            .as_array_mut()
            .ok_or("runtime-foundation asset lost provider array")?
            .iter_mut()
            .find(|record| record["state"].get("state").and_then(serde_json::Value::as_str) == Some("captured"))
            .ok_or("fixture lost captured provider state")?;
        captured["state"]["receipt"]["effects"]
            .as_object_mut()
            .ok_or("captured provider effects were not an object")?
            .remove("checked_cfg");
        assert!(serde_json::from_value::<OvenRuntimeFoundationAsset>(wire).is_err());
        Ok(())
    }

    /// Supported legacy and current Cargo directive spellings retain every compiler-relevant provider fact.
    #[test]
    fn runtime_foundation_provider_parser_retains_supported_directives() -> Result<(), Box<dyn std::error::Error>> {
        let directives = parse_runtime_foundation_provider_directives(
            b"cargo:rustc-cfg=provider_cfg\n\
cargo::rustc-check-cfg=cfg(provider_cfg)\n\
cargo:rustc-env=PROVIDER_VALUE=fixture-value\n\
cargo::rerun-if-changed=build.rs\n\
cargo:rerun-if-env-changed=PROVIDER_VALUE\n",
        )?;
        assert_eq!(directives.emitted_cfg, ["provider_cfg"]);
        assert_eq!(directives.checked_cfg, ["cfg(provider_cfg)"]);
        assert_eq!(
            directives.emitted_environment.get("PROVIDER_VALUE").map(String::as_str),
            Some("fixture-value")
        );
        assert_eq!(directives.rerun_paths, ["build.rs"]);
        assert_eq!(directives.rerun_environment, ["PROVIDER_VALUE"]);
        assert!(matches!(
            directives.native_link,
            OvenRuntimeFoundationNativeLinkState::NoNativeLink
        ));
        Ok(())
    }

    /// A native-link directive remains explicit provenance and cannot masquerade as an empty direct-Rustc link plan.
    #[test]
    fn runtime_foundation_provider_parser_marks_native_link_unsupported() -> Result<(), Box<dyn std::error::Error>> {
        let directives = parse_runtime_foundation_provider_directives(b"cargo:rustc-link-lib=ssl\n")?;
        assert!(matches!(
            directives.native_link,
            OvenRuntimeFoundationNativeLinkState::Unsupported { ref reason }
                if reason.contains("rustc-link-lib") && !reason.contains("ssl")
        ));
        Ok(())
    }

    /// Unsupported or malformed stdout cannot silently vanish before provider receipt sealing.
    #[test]
    fn runtime_foundation_provider_parser_refuses_unmodelled_output() {
        for stdout in [
            b"provider diagnostic\n".as_slice(),
            b"cargo:rustc-flags=-C target-cpu=native\n".as_slice(),
            b"cargo:rustc-env=PROVIDER_VALUE=first\ncargo::rustc-env=PROVIDER_VALUE=second\n".as_slice(),
        ] {
            assert!(parse_runtime_foundation_provider_directives(stdout).is_err());
        }
    }

    /// A repository-controlled zero-dependency script compiles and runs through direct Rustc with no Cargo runtime.
    #[test]
    fn runtime_foundation_provider_controlled_fixture_uses_direct_rustc() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("build.rs");
        fs::write(
            &source,
            r#"fn main() {
    assert!(std::env::var_os("CARGO").is_none());
    let out_dir = std::env::var("OUT_DIR").expect("owned OUT_DIR");
    let value = std::env::var("PROVIDER_VALUE").expect("explicit provider value");
    std::fs::write(
        std::path::Path::new(&out_dir).join("private.rs"),
        "pub const PROVIDER_MARKER: &str = \"fixture\";\n",
    )
    .expect("write generated source");
    println!("cargo:rustc-cfg=provider_cfg");
    println!("cargo::rustc-check-cfg=cfg(provider_cfg)");
    println!("cargo:rustc-env=PROVIDER_VALUE={value}");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed=PROVIDER_VALUE");
}
"#,
        )?;
        let rustc = crate::oven::rustc::resolve_active_rustc()?;
        let executable = root
            .path()
            .join(format!("provider-fixture{}", std::env::consts::EXE_SUFFIX));
        let mut compile = Command::new(&rustc);
        compile
            .args(["--edition=2024", "--crate-name", "provider_fixture"])
            .arg(&source)
            .arg("-o")
            .arg(&executable);
        crate::oven::rustc::clear_inherited_cargo_environment(&mut compile);
        let compiled = compile.output()?;
        if !compiled.status.success() {
            return Err(format!(
                "controlled provider fixture failed to compile with status {}",
                compiled.status
            )
            .into());
        }
        let out_dir = root.path().join("owned-out");
        fs::create_dir(&out_dir)?;
        let executed = Command::new(&executable)
            .current_dir(root.path())
            .env_clear()
            .env("OUT_DIR", &out_dir)
            .env("PROVIDER_VALUE", "fixture-value")
            .output()?;
        if !executed.status.success() {
            return Err(format!(
                "controlled provider fixture failed to run with status {}",
                executed.status
            )
            .into());
        }
        assert_eq!(
            fs::read(out_dir.join("private.rs"))?,
            b"pub const PROVIDER_MARKER: &str = \"fixture\";\n"
        );
        let directives = parse_runtime_foundation_provider_directives(&executed.stdout)?;
        assert_eq!(directives.emitted_cfg, ["provider_cfg"]);
        assert_eq!(directives.checked_cfg, ["cfg(provider_cfg)"]);
        assert_eq!(
            directives.emitted_environment.get("PROVIDER_VALUE").map(String::as_str),
            Some("fixture-value")
        );
        assert_eq!(directives.rerun_paths, ["build.rs"]);
        assert_eq!(directives.rerun_environment, ["PROVIDER_VALUE"]);
        let mut selected = foundation()?
            .selected_graph
            .units
            .into_iter()
            .find(|unit| unit.crate_name == "serde_derive")
            .ok_or("fixture lost cfg-only provider unit")?;
        selected.environment.insert(
            "PROVIDER_VALUE".to_string(),
            OvenSelectedRustFacetEnvironmentValue::Text {
                value: "fixture-value".to_string(),
            },
        );
        let declaration = fixture_build_script_declaration(&selected)?;
        let effects = bind_runtime_foundation_provider_directives(
            &selected,
            &declaration,
            &BTreeMap::from([(
                "PROVIDER_VALUE".to_string(),
                OvenMaterializedRustFacetEnvironmentValue::Text("fixture-value".to_string()),
            )]),
            Vec::new(),
            directives,
        )?;
        assert_eq!(effects.emitted_cfg, ["provider_cfg"]);
        assert_eq!(effects.checked_cfg, ["cfg(provider_cfg)"]);
        assert_eq!(
            effects
                .emitted_environment
                .get("PROVIDER_VALUE")
                .and_then(|value| match value {
                    OvenSelectedRustFacetEnvironmentValue::Text { value } => Some(value.as_str()),
                    OvenSelectedRustFacetEnvironmentValue::Path { .. }
                    | OvenSelectedRustFacetEnvironmentValue::SensitiveDigest { .. } => None,
                }),
            Some("fixture-value")
        );
        Ok(())
    }

    /// A raw value cannot become durable provider evidence when selection retained only a redacted digest.
    #[test]
    fn runtime_foundation_provider_binding_refuses_unadmitted_sensitive_value() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut selected = foundation()?
            .selected_graph
            .units
            .into_iter()
            .find(|unit| unit.crate_name == "serde_derive")
            .ok_or("fixture lost cfg-only provider unit")?;
        selected.environment.insert(
            "PRIVATE_VALUE".to_string(),
            OvenSelectedRustFacetEnvironmentValue::SensitiveDigest {
                hmac_sha256: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            },
        );
        let declaration = fixture_build_script_declaration(&selected)?;
        let directives =
            parse_runtime_foundation_provider_directives(b"cargo:rustc-env=PRIVATE_VALUE=fixture-secret\n")?;
        let error = bind_runtime_foundation_provider_directives(
            &selected,
            &declaration,
            &BTreeMap::from([(
                "PRIVATE_VALUE".to_string(),
                OvenMaterializedRustFacetEnvironmentValue::Text("fixture-secret".to_string()),
            )]),
            Vec::new(),
            directives,
        )
        .expect_err("redacted provider value must require a separate admitted value provider");
        assert!(matches!(
            error,
            OvenRustcError::InvalidInput {
                field: "runtime foundation provider environment",
                ..
            }
        ));
        assert!(!error.to_string().contains("fixture-secret"));
        Ok(())
    }

    /// A cfg-only build script cannot be relabelled as absent just because it generated no source file.
    #[test]
    fn runtime_foundation_asset_refuses_missing_cfg_only_provider_state() -> Result<(), Box<dyn std::error::Error>> {
        let mut asset = foundation_asset()?;
        let serde_derive_identity = asset
            .foundation
            .selected_graph
            .units
            .iter()
            .find(|unit| unit.crate_name == "serde_derive")
            .map(|unit| unit.identity.clone())
            .ok_or("fixture lost serde_derive")?;
        let record = asset
            .providers
            .iter_mut()
            .find(|record| record.selected_identity == serde_derive_identity)
            .ok_or("fixture lost serde_derive provider record")?;
        record.state = OvenRuntimeFoundationProviderState::NoProvider;
        asset.foundation_identity = runtime_foundation_asset_identity(&asset.foundation, &asset.providers)?;
        assert!(matches!(
            asset.validated(),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation providers",
                ..
            })
        ));
        Ok(())
    }

    /// A selected generated input cannot be silently relabelled as coming from no build script.
    #[test]
    fn runtime_foundation_asset_refuses_generated_input_without_build_script() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut asset = foundation_asset()?;
        let serde_identity = asset
            .foundation
            .selected_graph
            .units
            .iter()
            .find(|unit| unit.crate_name == "serde")
            .map(|unit| unit.identity.clone())
            .ok_or("fixture lost serde")?;
        let no_build_script_package = asset
            .foundation
            .selected_graph
            .units
            .iter()
            .find(|unit| unit.identity == serde_identity)
            .map(|unit| fixture_provider_package_source(unit, Vec::new()))
            .transpose()?
            .ok_or("fixture lost serde package source")?;
        let record = asset
            .providers
            .iter_mut()
            .find(|record| record.selected_identity == serde_identity)
            .ok_or("fixture lost serde provider record")?;
        record.declaration = OvenRuntimeFoundationProviderDeclaration::NoBuildScript {
            package: no_build_script_package,
        };
        record.state = OvenRuntimeFoundationProviderState::NoProvider;
        asset.foundation_identity = runtime_foundation_asset_identity(&asset.foundation, &asset.providers)?;
        assert!(matches!(
            asset.validated(),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation providers",
                ..
            })
        ));
        Ok(())
    }

    /// A declared build-script entrypoint must be an exact provider source member rather than an inferred path.
    #[test]
    fn runtime_foundation_asset_refuses_unselected_build_script_entrypoint() -> Result<(), Box<dyn std::error::Error>> {
        let mut asset = foundation_asset()?;
        let record = asset
            .providers
            .iter_mut()
            .find(|record| {
                matches!(
                    record.declaration,
                    OvenRuntimeFoundationProviderDeclaration::BuildScript { .. }
                )
            })
            .ok_or("fixture lost build-script provider record")?;
        let OvenRuntimeFoundationProviderDeclaration::BuildScript { entrypoint, .. } = &mut record.declaration else {
            return Err("fixture provider declaration changed shape".into());
        };
        *entrypoint = "not-selected.rs".to_string();
        asset.foundation_identity = runtime_foundation_asset_identity(&asset.foundation, &asset.providers)?;
        assert!(matches!(
            asset.validated(),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation provider build-script entrypoint",
                ..
            })
        ));
        Ok(())
    }

    /// A provider may request reruns only for a sealed member of its own complete package closure.
    #[test]
    fn runtime_foundation_asset_refuses_rerun_path_outside_provider_source_closure()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut asset = foundation_asset()?;
        let receipt = asset
            .providers
            .iter_mut()
            .find_map(|record| match &mut record.state {
                OvenRuntimeFoundationProviderState::Captured { receipt } => Some(receipt),
                OvenRuntimeFoundationProviderState::NoProvider
                | OvenRuntimeFoundationProviderState::Unsupported { .. } => None,
            })
            .ok_or("fixture lost captured provider receipt")?;
        receipt.effects.rerun_paths = vec!["unselected.rs".to_string()];
        seal_runtime_foundation_provider_receipts(&mut asset.providers)?;
        asset.foundation_identity = runtime_foundation_asset_identity(&asset.foundation, &asset.providers)?;

        assert!(matches!(
            asset.validated(),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation provider rerun paths",
                ..
            })
        ));
        Ok(())
    }

    /// The typed package manifest is a verified provider read/rerun input even though it is not a generic package
    /// member and therefore cannot become part of the compiler-visible unit identity by accident.
    #[test]
    fn runtime_foundation_asset_admits_manifest_rerun_path() -> Result<(), Box<dyn std::error::Error>> {
        let mut asset = foundation_asset()?;
        let serde_identity = asset
            .foundation
            .selected_graph
            .units
            .iter()
            .find(|unit| unit.crate_name == "serde")
            .map(|unit| unit.identity.clone())
            .ok_or("fixture lost serde")?;
        let receipt = asset
            .providers
            .iter_mut()
            .find(|record| record.selected_identity == serde_identity)
            .and_then(|record| match &mut record.state {
                OvenRuntimeFoundationProviderState::Captured { receipt } => Some(receipt),
                OvenRuntimeFoundationProviderState::NoProvider
                | OvenRuntimeFoundationProviderState::Unsupported { .. } => None,
            })
            .ok_or("fixture lost serde provider receipt")?;
        receipt.effects.rerun_paths = vec!["Cargo.toml".to_string()];
        seal_runtime_foundation_provider_receipts(&mut asset.providers)?;
        asset.foundation_identity = runtime_foundation_asset_identity(&asset.foundation, &asset.providers)?;

        let foundation_root = tempfile::tempdir()?;
        let toolchain_root = tempfile::tempdir()?;
        write_materialization_fixture(foundation_root.path(), toolchain_root.path())?;
        write_foundation_descriptor(foundation_root.path(), &asset)?;
        let materialized =
            admit_runtime_foundation_asset_for_publication(foundation_root.path(), toolchain_root.path())?
                .materialize_asset_for_publication()?;
        let build_script = materialized
            .provider_build_script(&serde_identity)
            .ok_or("fixture lost materialized serde build script")?;
        assert!(build_script.source_member("Cargo.toml").is_some());
        Ok(())
    }

    /// Provider receipt/effect changes participate in the release-asset identity even when source coordinates do not.
    #[test]
    fn runtime_foundation_asset_identity_covers_provider_effects() -> Result<(), Box<dyn std::error::Error>> {
        let mut asset = foundation_asset()?;
        let original = asset.foundation_identity.clone();
        let effects = asset
            .providers
            .iter_mut()
            .find_map(|record| match &mut record.state {
                OvenRuntimeFoundationProviderState::Captured { receipt } => Some(&mut receipt.effects),
                OvenRuntimeFoundationProviderState::NoProvider
                | OvenRuntimeFoundationProviderState::Unsupported { .. } => None,
            })
            .ok_or("fixture lost captured provider state")?;
        effects.checked_cfg.push("cfg(changed_provider_check)".to_string());
        assert!(matches!(
            asset.clone().validated(),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation asset identity",
                ..
            })
        ));
        asset.foundation_identity = runtime_foundation_asset_identity(&asset.foundation, &asset.providers)?;
        assert!(matches!(
            asset.clone().validated(),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation provider effect digest",
                ..
            })
        ));
        let asset = OvenRuntimeFoundationAsset::sealed(asset.foundation.clone(), asset.providers.clone())?;
        let asset = asset.validated()?;
        assert_ne!(asset.foundation_identity(), original);
        Ok(())
    }

    /// Policy, provider and selected-unit presentation order cannot manufacture a different foundation identity.
    #[test]
    fn runtime_foundation_asset_identity_canonicalizes_unordered_facts() -> Result<(), Box<dyn std::error::Error>> {
        let asset = foundation_asset()?;
        let expected = asset.foundation_identity.clone();
        let mut reordered = asset.clone();
        reordered.foundation.selected_graph.units.reverse();
        reordered.foundation.units.reverse();
        reordered.providers.reverse();
        reordered.foundation_identity = runtime_foundation_asset_identity(&reordered.foundation, &reordered.providers)?;
        assert_eq!(reordered.foundation_identity, expected);
        let validated = reordered.validated()?;
        assert_eq!(validated.foundation_identity(), expected);
        Ok(())
    }

    /// A captured provider may retain an unmodelled native-link fact for release provenance, but it cannot become a
    /// source-backed compiler foundation before a typed direct-Rustc link-plan field exists.
    #[test]
    fn runtime_foundation_asset_blocks_unsupported_native_link_materialization()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut asset = foundation_asset()?;
        let native_link = asset
            .providers
            .iter_mut()
            .find_map(|record| match &mut record.state {
                OvenRuntimeFoundationProviderState::Captured { receipt } => Some(&mut receipt.effects.native_link),
                OvenRuntimeFoundationProviderState::NoProvider
                | OvenRuntimeFoundationProviderState::Unsupported { .. } => None,
            })
            .ok_or("fixture lost captured provider state")?;
        *native_link = OvenRuntimeFoundationNativeLinkState::Unsupported {
            reason: "provider emitted rustc-link-lib".to_string(),
        };
        seal_runtime_foundation_provider_receipts(&mut asset.providers)?;
        asset.foundation_identity = runtime_foundation_asset_identity(&asset.foundation, &asset.providers)?;
        asset.clone().validated()?;
        let foundation_root = tempfile::tempdir()?;
        let toolchain_root = tempfile::tempdir()?;
        write_materialization_fixture(foundation_root.path(), toolchain_root.path())?;
        write_foundation_descriptor(foundation_root.path(), &asset)?;
        let admitted = admit_runtime_foundation_asset_for_publication(foundation_root.path(), toolchain_root.path())?;
        assert!(matches!(
            admitted.materialize_asset_for_publication(),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation providers",
                ..
            })
        ));
        Ok(())
    }

    /// The explicit release loader admits only the descriptor-derived foundation file set before materialization.
    #[test]
    fn runtime_foundation_asset_admits_exact_physical_members() -> Result<(), Box<dyn std::error::Error>> {
        let asset = foundation_asset()?;
        let foundation_root = tempfile::tempdir()?;
        let toolchain_root = tempfile::tempdir()?;
        write_materialization_fixture(foundation_root.path(), toolchain_root.path())?;
        write_foundation_descriptor(foundation_root.path(), &asset)?;

        let admitted = admit_runtime_foundation_asset_for_publication(foundation_root.path(), toolchain_root.path())?;
        assert_eq!(admitted.foundation_identity(), asset.foundation_identity);
        let materialized = admitted.materialize_asset_for_publication()?;
        assert_eq!(materialized.foundation_identity(), asset.foundation_identity);
        assert_eq!(
            materialized.foundation().compiler_closure_digest(),
            asset.foundation.compiler_closure_digest
        );
        let serde = materialized
            .foundation()
            .selected_graph()
            .graph()
            .units
            .iter()
            .find(|unit| unit.crate_name == "serde")
            .ok_or("fixture lost serde")?;
        assert!(matches!(
            materialized.provider_record(&serde.identity),
            Some(OvenRuntimeFoundationProviderRecord {
                declaration: OvenRuntimeFoundationProviderDeclaration::BuildScript { entrypoint, .. },
                state: OvenRuntimeFoundationProviderState::Captured { .. },
                ..
            }) if entrypoint == "build.rs"
        ));
        let build_script = materialized
            .provider_build_script(&serde.identity)
            .ok_or("fixture lost materialized provider build script")?;
        assert!(build_script.source_root().ends_with("registry-sources/serde-1.0.0"));
        assert!(build_script.entrypoint().ends_with("build.rs"));
        assert_eq!(build_script.source_member("build.rs"), Some(build_script.entrypoint()));
        assert_eq!(materialized.materialized().rebuild_order().count(), 2);
        Ok(())
    }

    /// A provider publisher copies only descriptor-derived members, re-admits the staged payload, and exposes the
    /// completed foundation atomically at its requested destination.
    #[test]
    fn runtime_foundation_asset_publisher_writes_only_declared_members() -> Result<(), Box<dyn std::error::Error>> {
        let asset = foundation_asset()?;
        let source_root = tempfile::tempdir()?;
        let toolchain_root = tempfile::tempdir()?;
        let install_root = tempfile::tempdir()?;
        write_materialization_fixture(source_root.path(), toolchain_root.path())?;
        write_fixture_file(source_root.path(), "ambient/target-cache.bin", b"must not publish")?;
        let destination = install_root.path().join("runtime-foundation");

        let admitted =
            publish_runtime_foundation_asset(asset.clone(), source_root.path(), toolchain_root.path(), &destination)?;
        assert_eq!(admitted.foundation_identity(), asset.foundation_identity);
        assert_eq!(
            admitted
                .materialize_asset_for_publication()?
                .materialized()
                .rebuild_order()
                .count(),
            2
        );
        assert!(!destination.join("ambient/target-cache.bin").exists());
        assert!(destination.join(OVEN_RUNTIME_FOUNDATION_ASSET_FILENAME).is_file());
        assert_eq!(
            fs::read_dir(install_root.path())?.collect::<Result<Vec<_>, _>>()?.len(),
            1,
            "a completed asset leaves no visible staging sibling"
        );
        Ok(())
    }

    /// A changed source byte is rejected before the publisher creates a visible asset or leaves a private staging
    /// root behind.
    #[test]
    fn runtime_foundation_asset_publisher_refuses_changed_source_before_exposure()
    -> Result<(), Box<dyn std::error::Error>> {
        let asset = foundation_asset()?;
        let source_root = tempfile::tempdir()?;
        let toolchain_root = tempfile::tempdir()?;
        let install_root = tempfile::tempdir()?;
        write_materialization_fixture(source_root.path(), toolchain_root.path())?;
        fs::write(source_root.path().join("deps/libserde.rlib"), b"changed rlib")?;
        let destination = install_root.path().join("runtime-foundation");

        assert!(matches!(
            publish_runtime_foundation_asset(asset, source_root.path(), toolchain_root.path(), &destination),
            Err(OvenRustcError::ArtifactDigestMismatch { .. })
        ));
        assert!(matches!(
            fs::symlink_metadata(&destination),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound
        ));
        assert!(fs::read_dir(install_root.path())?.next().is_none());
        Ok(())
    }

    /// Provider-only source bytes are verified before publishing and cannot be replaced while the compiled Rust unit
    /// itself remains reusable.
    #[test]
    fn runtime_foundation_asset_publisher_refuses_changed_provider_source_before_exposure()
    -> Result<(), Box<dyn std::error::Error>> {
        let asset = foundation_asset()?;
        let source_root = tempfile::tempdir()?;
        let toolchain_root = tempfile::tempdir()?;
        let install_root = tempfile::tempdir()?;
        write_materialization_fixture(source_root.path(), toolchain_root.path())?;
        fs::write(
            source_root.path().join("registry-sources/serde-1.0.0/build.rs"),
            b"fn main() { println!(\"changed provider\"); }\n",
        )?;
        let destination = install_root.path().join("runtime-foundation");

        assert!(matches!(
            publish_runtime_foundation_asset(asset, source_root.path(), toolchain_root.path(), &destination),
            Err(OvenRustcError::ArtifactDigestMismatch { .. })
        ));
        assert!(matches!(
            fs::symlink_metadata(&destination),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound
        ));
        assert!(fs::read_dir(install_root.path())?.next().is_none());
        Ok(())
    }

    /// Existing installation content is immutable to a publisher: a retry must fail rather than replacing a
    /// possibly leased asset.
    #[test]
    fn runtime_foundation_asset_publisher_refuses_existing_destination() -> Result<(), Box<dyn std::error::Error>> {
        let asset = foundation_asset()?;
        let source_root = tempfile::tempdir()?;
        let toolchain_root = tempfile::tempdir()?;
        let install_root = tempfile::tempdir()?;
        write_materialization_fixture(source_root.path(), toolchain_root.path())?;
        let destination = install_root.path().join("runtime-foundation");
        fs::create_dir(&destination)?;
        fs::write(destination.join("sentinel"), b"existing asset remains untouched")?;

        assert!(matches!(
            publish_runtime_foundation_asset(asset, source_root.path(), toolchain_root.path(), &destination),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation asset destination",
                ..
            })
        ));
        assert_eq!(
            fs::read(destination.join("sentinel"))?,
            b"existing asset remains untouched"
        );
        Ok(())
    }

    /// A release payload cannot smuggle a file beside otherwise valid source and artifact members.
    #[test]
    fn runtime_foundation_asset_refuses_undeclared_physical_member() -> Result<(), Box<dyn std::error::Error>> {
        let asset = foundation_asset()?;
        let foundation_root = tempfile::tempdir()?;
        let toolchain_root = tempfile::tempdir()?;
        write_materialization_fixture(foundation_root.path(), toolchain_root.path())?;
        write_foundation_descriptor(foundation_root.path(), &asset)?;
        fs::write(foundation_root.path().join("unrecorded.bin"), b"not in the descriptor")?;

        assert!(matches!(
            admit_runtime_foundation_asset_for_publication(foundation_root.path(), toolchain_root.path()),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation asset members",
                ..
            })
        ));
        Ok(())
    }

    /// A provider source closure remains closed even when the unrelated selected unit source tree is otherwise valid.
    #[test]
    fn runtime_foundation_asset_refuses_undeclared_provider_source_member() -> Result<(), Box<dyn std::error::Error>> {
        let asset = foundation_asset()?;
        let foundation_root = tempfile::tempdir()?;
        let toolchain_root = tempfile::tempdir()?;
        write_materialization_fixture(foundation_root.path(), toolchain_root.path())?;
        write_foundation_descriptor(foundation_root.path(), &asset)?;
        write_fixture_file(
            foundation_root.path(),
            "registry-sources/serde-1.0.0/provider-unrecorded.rs",
            b"fn hidden_provider_input() {}\n",
        )?;

        assert!(matches!(
            admit_runtime_foundation_asset_for_publication(foundation_root.path(), toolchain_root.path()),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation asset members",
                ..
            })
        ));
        Ok(())
    }

    /// An empty directory is still release content and cannot hide beside a descriptor-derived asset tree.
    #[test]
    fn runtime_foundation_asset_refuses_undeclared_empty_directory() -> Result<(), Box<dyn std::error::Error>> {
        let asset = foundation_asset()?;
        let foundation_root = tempfile::tempdir()?;
        let toolchain_root = tempfile::tempdir()?;
        write_materialization_fixture(foundation_root.path(), toolchain_root.path())?;
        write_foundation_descriptor(foundation_root.path(), &asset)?;
        fs::create_dir(foundation_root.path().join("unrecorded-empty"))?;

        assert!(matches!(
            admit_runtime_foundation_asset_for_publication(foundation_root.path(), toolchain_root.path()),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation asset members",
                ..
            })
        ));
        Ok(())
    }

    /// The recursive audit refuses links instead of letting a descriptor-derived name escape the asset root.
    #[cfg(unix)]
    #[test]
    fn runtime_foundation_asset_refuses_symlinked_member() -> Result<(), Box<dyn std::error::Error>> {
        let asset = foundation_asset()?;
        let foundation_root = tempfile::tempdir()?;
        let toolchain_root = tempfile::tempdir()?;
        write_materialization_fixture(foundation_root.path(), toolchain_root.path())?;
        write_foundation_descriptor(foundation_root.path(), &asset)?;
        std::os::unix::fs::symlink(
            foundation_root.path().join("deps/libserde.rlib"),
            foundation_root.path().join("linked-artifact"),
        )?;

        assert!(matches!(
            admit_runtime_foundation_asset_for_publication(foundation_root.path(), toolchain_root.path()),
            Err(OvenRustcError::InvalidInput {
                field: "runtime foundation asset members",
                ..
            })
        ));
        Ok(())
    }

    /// Publisher materialization admits only the sealed physical foundation and returns direct-Rustc inputs in
    /// compiler-owned dependency order without consulting a Cargo manifest, cache, or registry.
    #[test]
    fn runtime_foundation_materializes_sealed_direct_rustc_inputs() -> Result<(), Box<dyn std::error::Error>> {
        let foundation = foundation()?.validated()?;
        let graph = foundation.selected_graph().graph();
        let core = graph
            .units
            .iter()
            .find(|unit| unit.crate_name == "incan_core")
            .ok_or("fixture lost incan_core")?;
        let stdlib = graph
            .units
            .iter()
            .find(|unit| unit.crate_name == "incan_stdlib")
            .ok_or("fixture lost incan_stdlib")?;
        let foundation_root = tempfile::tempdir()?;
        let toolchain_root = tempfile::tempdir()?;

        write_foundation_materialization_fixture(foundation_root.path(), toolchain_root.path())?;

        let roots = materialization_owner_roots(foundation_root.path(), toolchain_root.path());
        let materialized = foundation.materialize_for_publication(&roots)?;
        let order = materialized.rebuild_order().collect::<Vec<_>>();
        assert_eq!(order, vec![core.identity.as_str(), stdlib.identity.as_str()]);
        assert!(materialized.sources().unit(&core.identity).is_some());
        assert!(materialized.sources().unit(&stdlib.identity).is_some());
        assert_eq!(materialized.artifact_plan().externs.len(), 2);

        let dependencies = materialized
            .prebuilt_dependencies(&core.identity)
            .ok_or("fixture did not project core prebuilt dependencies")?;
        assert_eq!(dependencies.len(), 2);
        let serde = dependencies
            .iter()
            .find(|dependency| dependency.alias == "serde")
            .ok_or("fixture did not project serde")?;
        assert_eq!(serde.domain, OvenSelectedRustFacetDomain::Target);
        assert_eq!(
            serde.artifact,
            fs::canonicalize(foundation_root.path().join("deps/libserde.rlib"))?
        );
        let serde_derive = dependencies
            .iter()
            .find(|dependency| dependency.alias == "serde_derive")
            .ok_or("fixture did not project serde_derive")?;
        assert_eq!(serde_derive.domain, OvenSelectedRustFacetDomain::Host);
        assert_eq!(
            serde_derive.artifact,
            fs::canonicalize(foundation_root.path().join("deps/libserde_derive.dylib"))?
        );
        Ok(())
    }

    /// Publication-time materialization must rehash an SDK artifact before the later trusted plan can expose it.
    #[test]
    fn runtime_foundation_refuses_mutated_artifact_during_publication_materialization()
    -> Result<(), Box<dyn std::error::Error>> {
        let foundation = foundation()?.validated()?;
        let foundation_root = tempfile::tempdir()?;
        let toolchain_root = tempfile::tempdir()?;
        write_foundation_materialization_fixture(foundation_root.path(), toolchain_root.path())?;
        fs::write(
            foundation_root.path().join("deps/libserde.rlib"),
            b"substituted artifact",
        )?;
        let roots = materialization_owner_roots(foundation_root.path(), toolchain_root.path());
        assert!(matches!(
            foundation.materialize_for_publication(&roots),
            Err(OvenRustcError::ArtifactDigestMismatch { .. })
        ));
        Ok(())
    }
}
