//! Replacement compatibility control-plane collector.
//!
//! The public `std.features` registry remains the authority for user-facing capability descriptions. This module
//! collects feature and implementation-requirement registrations from the compiler boundaries that own them. During
//! the 0.5-to-replacement migration, it also carries a deliberately temporary bootstrap crosswalk for work that has
//! not yet reached an owning implementation boundary. The collector makes that debt and its retirement conditions
//! visible; it is not a permanent second catalogue of language features.
//!
//! The control plane moves a frozen release baseline through Body IR, direct replacement execution, and independent
//! comparison without collapsing those facts into one traffic-light status. It intentionally contains no executor
//! implementation and cannot change backend selection.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use thiserror::Error;

use crate::frontend::feature_metadata::{PublicFeatureDescriptor, public_feature_descriptors};

const BASELINE_MANIFEST: &str = include_str!("replacement_compatibility/migration_baselines/v0.5.0/manifest.json");
const FROZEN_V0_5_CAPABILITIES_SOURCE: &[u8] =
    include_bytes!("replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn");
const FROZEN_V0_5_CAPABILITIES_PATH: &str =
    "src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn";
const LIVE_FEATURES_SOURCE: &str = "crates/incan_stdlib/stdlib/features.incn";

/// The same catalogue's path *at the v0.5.0 tag*, before the #1228 rename.
///
/// The release pin is verified by reading the blob out of that tag, so it has to name the file as that tag spells it.
/// Using the live path here would silently find nothing and pin against an empty result.
#[cfg(test)]
const FEATURES_SOURCE_AT_V0_5: &str = "crates/incan_stdlib/stdlib/capabilities.incn";
const COMPLETED_COMPARISON_INFRASTRUCTURE_ISSUE: u32 = 1146;

/// Version of the machine-readable replacement compatibility inventory document.
///
/// Bump this whenever the serialized document's field shape or a serialized enum contract changes. The public
/// `std.features` registry remains independently versioned and is not governed by this projection version.
pub const REPLACEMENT_COMPATIBILITY_INVENTORY_SCHEMA_VERSION: u32 = 4;

/// Lifecycle of one input to the replacement compatibility collector.
///
/// A local registration is durable compiler architecture: the boundary that implements a feature or private
/// mechanism declares its evidence alongside that implementation. A migration bootstrap registration is temporary
/// coverage scaffolding and must name its retirement condition. The collector can contain both while the migration
/// is incomplete, but cannot hide one as the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum CompatibilityRegistrationLifecycle {
    /// A compiler module owns this registration beside the implementation boundary it describes.
    LocalImplementation,
    /// A temporary release-migration crosswalk awaits migration into local implementation registrations.
    MigrationBootstrap,
}

impl CompatibilityRegistrationLifecycle {
    /// Return the stable developer-facing spelling for projections and reports.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LocalImplementation => "LocalImplementation",
            Self::MigrationBootstrap => "MigrationBootstrap",
        }
    }
}

/// Provenance for a set of records collected into the replacement compatibility inventory.
///
/// This is intentionally a module-level record rather than a tag on every helper function. A source-observable
/// feature can span type checking, Body IR, execution, and comparison; the nearest boundary that owns each coherent
/// contribution registers it here, and the collector validates the joined result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompatibilityRegistrationSource {
    /// Stable identity of the contributing compiler boundary.
    pub id: String,
    /// Whether this contributor is durable local architecture or temporary migration scaffolding.
    pub lifecycle: CompatibilityRegistrationLifecycle,
    /// Repository-relative module or fixture path containing the registration.
    pub repository_path: String,
    /// Stable source selector for the registration function or bootstrap declaration.
    pub selector: String,
    /// Explicit exit condition for migration-only scaffolding; absent for local implementation registrations.
    pub retirement_condition: Option<String>,
    /// Stable compatibility-feature IDs supplied by this contributor.
    pub feature_ids: Vec<String>,
    /// Stable private-requirement IDs supplied by this contributor.
    pub requirement_ids: Vec<String>,
}

/// One module-owned contribution that the collector joins into the public registry projection.
///
/// This crate-private input type deliberately has no serialization contract. The collected registry is the
/// developer-facing projection; feature ownership remains local to the contributing compiler boundary.
#[derive(Debug, Clone)]
pub(crate) struct ReplacementCompatibilityContribution {
    pub(crate) source: CompatibilityRegistrationSource,
    pub(crate) features: Vec<CompatibilityFeature>,
    pub(crate) requirements: Vec<ImplementationRequirement>,
    pub(crate) feature_links: Vec<PublicFeatureLink>,
    pub(crate) requirement_links: Vec<FeatureRequirementLink>,
}

/// Purpose of a frozen release source in the compatibility collector.
///
/// This is intentionally not a general historical-registry taxonomy. A new release snapshot needs an explicit
/// migration use case; the normal source of truth remains the present-tense checked `std.features` registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReleaseBaselineRole {
    /// A temporary coverage ruler for the 0.5-to-replacement migration.
    MigrationCompatibilityTarget,
}

impl ReleaseBaselineRole {
    /// Return the stable developer-facing spelling for projections and reports.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MigrationCompatibilityTarget => "MigrationCompatibilityTarget",
        }
    }
}

/// Machine-readable pin for a released public-capability migration baseline.
///
/// The pin intentionally stores release identity and the Git object ID of the authored source, rather than a second
/// hand-maintained list of capability descriptors. The descriptor rows are derived from compiler-checked metadata
/// only after the source bytes match this pin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleasePin {
    /// Why this frozen source exists; this prevents a compatibility fixture from becoming an accidental archive.
    pub role: ReleaseBaselineRole,
    /// Semantic version tag that owns this immutable compatibility target.
    pub tag: String,
    /// Commit that carries the released `std.features` source.
    pub revision: String,
    /// Git blob ID for the exact authored capability-registry source bytes.
    pub source_blob: String,
    /// Repository-relative migration-fixture path containing the pinned source bytes.
    pub source_snapshot_path: String,
    /// Number of descriptors that the checked release source must decode to.
    pub expected_descriptor_count: usize,
    /// Explicit condition for retiring this baseline from the active control plane.
    pub retirement_condition: String,
}

/// A public capability record produced from checked `std.features` metadata.
///
/// This is a snapshot projection, not a writable replacement for the Incan-authored public registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublicFeatureRecord {
    /// Stable `FeatureId` selected by the Incan-authored registry.
    pub id: String,
    /// User-facing capability name.
    pub name: String,
    /// User-facing inventory category.
    pub category: String,
    /// First release that advertises the capability.
    pub since: String,
    /// Linked RFC identifier retained from the checked descriptor.
    pub rfc: String,
    /// Public stability classification.
    pub stability: String,
    /// Public activation contract.
    pub activation: String,
    /// Public capability summary.
    pub summary: String,
    /// Checked canonical source forms.
    pub canonical_forms: Vec<String>,
    /// Preferred public alternative from the descriptor.
    pub prefer_over: String,
    /// Checked public reference labels and paths.
    pub references: Vec<(String, String)>,
    /// Release and historical provenance state for this frozen public-capability record.
    pub landing_provenance: LandingProvenance,
}

/// Release-pinned public-capability membership plus the checked records that supply its fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublicCapabilityBaseline {
    /// Immutable release/source identity used to reject silent target drift.
    pub release: ReleasePin,
    /// Every public descriptor decoded from the pinned checked metadata source.
    pub capabilities: Vec<PublicFeatureRecord>,
}

/// Historical landing-evidence state for one frozen public capability.
///
/// A release descriptor proves that the release advertised a capability. It does not on its own prove a complete
/// original RFC, issue, PR, or merge trail, so unresolved discrepancies stay explicit rather than being promoted to
/// a fabricated historical assertion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum LandingProvenanceState {
    /// The frozen release descriptor is the currently audited landing evidence.
    ReleaseRegistryDeclared,
    /// The registry/RFC history disagrees or lacks a durable landing record and needs owned follow-up.
    HistoricalDiscrepancyUnresolved,
}

impl LandingProvenanceState {
    /// Return the stable developer-facing spelling for projections and reports.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReleaseRegistryDeclared => "ReleaseRegistryDeclared",
            Self::HistoricalDiscrepancyUnresolved => "HistoricalDiscrepancyUnresolved",
        }
    }
}

/// Typed landing evidence and the owner of any unresolved historical discrepancy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LandingProvenance {
    /// Whether released metadata or an explicitly owned discrepancy currently describes the evidence.
    pub state: LandingProvenanceState,
    /// Checked-source anchor containing the released capability descriptor.
    pub anchor: EvidenceAnchor,
    /// Issue responsible for resolving a historical discrepancy, when one exists.
    pub owner_issue: Option<u32>,
    /// Visible explanation for an unresolved discrepancy; absent only for release-registry declarations.
    pub note: Option<String>,
}

/// Closed compiler surface represented by an evidence anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum EvidenceSurface {
    /// Authored source or parsed source/AST contract.
    SourceAst,
    /// Typechecker acceptance, resolution, or diagnostic boundary.
    Typechecker,
    /// Lowered Body IR representation boundary.
    BodyIr,
    /// Direct replacement execution or explicit refusal boundary.
    ReplacementExecutor,
    /// #987 corpus case or owned plan boundary.
    ParityCorpus,
    /// #1146 receipt-bound independent comparison boundary.
    IndependentComparison,
}

impl EvidenceSurface {
    /// Return the stable developer-facing spelling for projections and reports.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SourceAst => "SourceAst",
            Self::Typechecker => "Typechecker",
            Self::BodyIr => "BodyIr",
            Self::ReplacementExecutor => "ReplacementExecutor",
            Self::ParityCorpus => "ParityCorpus",
            Self::IndependentComparison => "IndependentComparison",
        }
    }
}

/// Maturity of one typed source, compiler, runtime, or corpus anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum EvidenceAnchorStatus {
    /// The checked repository location and selector currently exist and support the stated fact.
    Observed,
    /// The owner must materialize the named anchor at the stated existing implementation boundary.
    Planned,
    /// The surface deliberately does not apply and carries an explicit boundary explanation.
    NotApplicable,
}

impl EvidenceAnchorStatus {
    /// Return the stable developer-facing spelling for projections and reports.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Observed => "Observed",
            Self::Planned => "Planned",
            Self::NotApplicable => "NotApplicable",
        }
    }
}

/// Actionable, typed evidence location in this repository.
///
/// `repository_path` is relative to the repository root and `selector` is a durable symbol, test, capability ID, or
/// case ID. Observed anchors are checked against the working tree by registry validation. Planned anchors still name
/// a real implementation boundary and must identify their owner; they never count as direct or comparison proof.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvidenceAnchor {
    /// Compiler surface supplied by this anchor.
    pub surface: EvidenceSurface,
    /// Whether the anchor is observed, planned, or deliberately not applicable.
    pub status: EvidenceAnchorStatus,
    /// Repository-relative source, implementation, or test path.
    pub repository_path: String,
    /// Symbol, stable case ID, or descriptor identity at the path.
    pub selector: String,
    /// Owning issue for a planned anchor; observed and non-applicable anchors have no owner here.
    pub owner_issue: Option<u32>,
    /// Explicit fact or non-applicability explanation.
    pub note: String,
}

/// Complete typed surface coverage for one compatibility feature.
///
/// The six fields intentionally make omission impossible in the canonical record. A boundary that does not execute
/// from Body IR still uses a typed `NotApplicable` anchor rather than disappearing from the inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FeatureSurfaceCoverage {
    /// Source or AST contract evidence.
    pub source_ast: EvidenceAnchor,
    /// Typechecker acceptance or diagnostic evidence.
    pub typechecker: EvidenceAnchor,
    /// Body IR representation or explicit non-applicability evidence.
    pub body_ir: EvidenceAnchor,
    /// Direct replacement execution/refusal or explicit planned boundary.
    pub replacement_executor: EvidenceAnchor,
    /// Stable #987 corpus link or typed corpus-plan identifier.
    pub parity_corpus: ParityCorpusReference,
    /// Aggregate receipt-bound comparison evidence or an explicit incomplete-coverage route.
    pub independent_comparison: ComparisonEvidence,
    /// Case-scoped comparison facts that must not widen the enclosing feature's aggregate comparison state.
    pub scoped_comparisons: Vec<CorpusCaseComparisonEvidence>,
}

/// Receipt-bound comparison fact for one stable #987 corpus case.
///
/// A matching case proves only that exact profile. In particular, it never promotes an enclosing feature whose
/// remaining corpus cases or source forms are still uncovered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CorpusCaseComparisonEvidence {
    /// Stable registered #987 case identifier.
    pub case_id: String,
    /// Independent comparison outcome for precisely this corpus case.
    pub state: IndependentComparisonState,
    /// Receipt-bound comparison anchors for precisely this corpus case.
    pub evidence: ComparisonEvidence,
}

/// Typed #987 corpus relationship for a compatibility feature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum ParityCorpusReference {
    /// Existing stable #987 corpus cases cover the bounded direct profile.
    Registered {
        /// Stable corpus IDs that are checked against the registered seed set.
        case_ids: Vec<String>,
        /// Test anchor that registers the corpus cases.
        anchor: EvidenceAnchor,
    },
    /// A future #987 case is reserved with a stable ID and explicitly owned materialization boundary.
    Planned {
        /// Reserved stable `parity-987-plan-*` case identifier.
        case_id: String,
        /// #987 owns corpus materialization.
        owner_issue: u32,
        /// Existing corpus extension point where the case must land.
        anchor: EvidenceAnchor,
    },
}

/// Completed comparison infrastructure that makes receipt-bound comparison possible.
///
/// This provenance is distinct from the owner of any still-missing case or aggregate evidence. In particular,
/// completion of the infrastructure issue never makes that completed issue an owner of outstanding evidence debt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompletedComparisonInfrastructure {
    /// Completed issue that delivered the reusable comparison route.
    pub issue: u32,
    /// Observed repository anchor proving the completed infrastructure boundary.
    pub anchor: EvidenceAnchor,
}

/// Ownership state for comparison evidence that remains unavailable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum OutstandingComparisonEvidence {
    /// A current feature/runtime owner is scheduled to materialize the missing comparison evidence.
    Scheduled {
        /// Open issue responsible for the remaining evidence, not the completed infrastructure issue.
        owner_issue: u32,
        /// Concrete scope of the still-missing evidence.
        note: String,
    },
    /// No feature/runtime owner has scheduled the remaining comparison evidence yet.
    UnscheduledDebt {
        /// Concrete reason that the missing evidence remains explicitly unscheduled.
        note: String,
    },
}

/// Typed independent-comparison evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum ComparisonEvidence {
    /// The aggregate or case remains non-green while completed comparison infrastructure awaits more evidence.
    Unavailable {
        /// Completed reusable comparison infrastructure that supplies provenance, not outstanding-work ownership.
        comparison_infrastructure: CompletedComparisonInfrastructure,
        /// Scheduled owner or explicit unscheduled debt for the still-missing evidence.
        outstanding_evidence: OutstandingComparisonEvidence,
    },
    /// A direct run and legacy run have matching receipt-bound comparison records.
    Paired {
        /// Receipt record for the legacy source-observable run.
        legacy_receipt: EvidenceAnchor,
        /// Receipt record for direct replacement execution.
        replacement_receipt: EvidenceAnchor,
        /// Comparison record that joins the two receipt identities.
        comparison_record: EvidenceAnchor,
    },
}

/// Whether the source-level public contract has an evidence anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum SourceContractState {
    /// The contract is represented by checked public metadata or a checked source probe.
    Checked,
    /// The contract still needs a source-level probe or classification.
    Planned,
}

impl SourceContractState {
    /// Return the stable developer-facing spelling for projections and reports.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Checked => "Checked",
            Self::Planned => "Planned",
        }
    }
}

/// What evidence currently establishes legacy execution for a compatibility feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum LegacyRunState {
    /// A source-observable legacy result is recorded for a named probe.
    Observed,
    /// The source contract exists, but no receipt-bound legacy run is recorded yet.
    Unknown,
    /// Legacy execution does not apply to this compiler or tooling control-plane contract.
    NotApplicable,
}

impl LegacyRunState {
    /// Return the stable developer-facing spelling for projections and reports.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Observed => "Observed",
            Self::Unknown => "Unknown",
            Self::NotApplicable => "NotApplicable",
        }
    }
}

/// How far Body IR currently represents the feature's source semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum BodyIrRepresentationState {
    /// Body IR has an explicit representation for the relevant source contract.
    Represented,
    /// Only some forms are represented; the distinction must remain visible.
    Partial,
    /// The feature is outside Body IR, such as a package or tooling boundary.
    NotApplicable,
    /// Body IR vocabulary or lowering is still required.
    Missing,
}

impl BodyIrRepresentationState {
    /// Return the stable developer-facing spelling for projections and reports.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Represented => "Represented",
            Self::Partial => "Partial",
            Self::NotApplicable => "NotApplicable",
            Self::Missing => "Missing",
        }
    }
}

/// Direct replacement-backend outcome for one source-observable feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum DirectReplacementOutcome {
    /// A bounded probe executes directly from Body IR and has direct-execution evidence.
    Executable,
    /// The current profile deliberately refuses this feature at admission or execution.
    ExplicitlyRefused,
    /// The feature has not been admitted because named implementation requirements remain incomplete.
    BlockedByRequirements,
    /// The feature is a non-Body-IR control-plane contract rather than a direct execution profile.
    OutsideDirectExecution,
}

impl DirectReplacementOutcome {
    /// Return the stable developer-facing spelling for projections and reports.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Executable => "Executable",
            Self::ExplicitlyRefused => "ExplicitlyRefused",
            Self::BlockedByRequirements => "BlockedByRequirements",
            Self::OutsideDirectExecution => "OutsideDirectExecution",
        }
    }
}

/// Independent source-observable comparison state.
///
/// This is deliberately separate from direct execution. A lowering snapshot, generated Rust build, or legacy compile
/// cannot manufacture a green result because none of those states is [`Self::ComparedMatch`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum IndependentComparisonState {
    /// This aggregate or corpus case lacks a paired Oven-owned legacy route, so it remains explicitly non-green.
    NonGreenShadowUnavailable,
    /// Matching receipts produced a source-observable comparison mismatch.
    ComparedMismatch,
    /// Matching receipts produced an agreeing source-observable comparison.
    ComparedMatch,
    /// The feature is not an execution profile and therefore has no comparison lane.
    NotApplicable,
}

impl IndependentComparisonState {
    /// Return the stable developer-facing spelling for projections and reports.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NonGreenShadowUnavailable => "NonGreenShadowUnavailable",
            Self::ComparedMismatch => "ComparedMismatch",
            Self::ComparedMatch => "ComparedMatch",
            Self::NotApplicable => "NotApplicable",
        }
    }
}

/// The release-level disposition of an in-baseline source-observable feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum CompatibilityDisposition {
    /// The registry has not classified this feature yet; validation must reject this placeholder.
    Unclassified,
    /// The bounded direct profile preserves the named contract, without asserting independent comparison green.
    Preserved,
    /// The source contract remains in scope and has an implementation owner.
    Planned,
    /// The source contract is in scope but blocked on named requirements or decisions.
    Blocked,
    /// A closed taxonomy excludes a non-execution control-plane concern from direct Body-IR execution.
    OutOfEnvelope,
}

impl CompatibilityDisposition {
    /// Return the stable developer-facing spelling for projections and reports.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unclassified => "Unclassified",
            Self::Preserved => "Preserved",
            Self::Planned => "Planned",
            Self::Blocked => "Blocked",
            Self::OutOfEnvelope => "OutOfEnvelope",
        }
    }
}

/// One named, structured source probe family, including both expected acceptance and refusal behavior.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceProbe {
    /// Stable probe-family identity, suitable for a future #987 corpus row or focused fixture.
    pub id: String,
    /// Expected accepted source-observable result or effect.
    pub positive: ProbeExpectation,
    /// Expected refusal or source diagnostic preventing a permissive false positive.
    pub negative: ProbeExpectation,
}

/// Structured expected outcome for one source probe direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ProbeOutcome {
    /// The probe must typecheck and observe the named source-level behavior.
    AcceptedBehavior,
    /// The probe must refuse with an intentional source-owned diagnostic or selection disposition.
    IntentionalRefusal,
}

impl ProbeOutcome {
    /// Return the stable developer-facing spelling for projections and reports.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AcceptedBehavior => "AcceptedBehavior",
            Self::IntentionalRefusal => "IntentionalRefusal",
        }
    }
}

/// Expected probe result paired with an actionable source or typechecker anchor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProbeExpectation {
    /// Closed expected-outcome classification.
    pub outcome: ProbeOutcome,
    /// Observable contract that the probe must establish.
    pub contract: String,
    /// Existing or planned source/typechecker location that materializes the probe.
    pub anchor: EvidenceAnchor,
}

/// Factored evidence for one compatibility feature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompatibilityEvidence {
    /// Source-contract evidence independent of execution results.
    pub source_contract: SourceContractState,
    /// Receipt-bound legacy-run evidence, if any.
    pub legacy_run: LegacyRunState,
    /// Body-IR representation evidence independent of execution results.
    pub body_ir: BodyIrRepresentationState,
    /// Direct replacement outcome independent of comparison state.
    pub direct_replacement: DirectReplacementOutcome,
    /// Independent comparison result; only this lane can make execution parity green.
    pub independent_comparison: IndependentComparisonState,
    /// Typed anchors across every relevant source, compiler, direct-execution, corpus, and comparison surface.
    pub surfaces: FeatureSurfaceCoverage,
}

impl CompatibilityEvidence {
    /// Whether evidence meets the narrow definition of a comparison-green direct-execution row.
    ///
    /// This intentionally ignores source acceptance, Body IR representation, generated Rust, and a legacy compiler
    /// result on their own. A future matcher must supply [`IndependentComparisonState::ComparedMatch`] backed by
    /// paired evidence before this returns `true`.
    pub const fn is_parity_green(&self) -> bool {
        matches!(self.direct_replacement, DirectReplacementOutcome::Executable)
            && matches!(self.legacy_run, LegacyRunState::Observed)
            && matches!(self.independent_comparison, IndependentComparisonState::ComparedMatch)
            && matches!(self.surfaces.independent_comparison, ComparisonEvidence::Paired { .. })
    }
}

/// A stable source-observable compatibility contract owned by the compiler control plane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompatibilityFeature {
    /// Stable compiler-owned compatibility-feature identity.
    pub id: String,
    /// User-observable behavior the replacement path must preserve or deliberately classify.
    pub contract: String,
    /// Positive and negative source probe families that make the contract testable.
    pub probes: Vec<SourceProbe>,
    /// Factored source, legacy, Body-IR, direct, and comparison facts.
    pub evidence: CompatibilityEvidence,
    /// Release disposition that must not be inferred from any one evidence lane.
    pub disposition: CompatibilityDisposition,
    /// Existing implementation issue that owns planned or blocked in-envelope work.
    pub owner_issue: Option<u32>,
    /// Implementation, migration, or blocker note required for non-preserved in-envelope work.
    pub migration_or_blocker: Option<String>,
}

/// A private compiler mechanism required by one or more compatibility features.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImplementationRequirement {
    /// Stable compiler-owned requirement identity.
    pub id: &'static str,
    /// Invariant the mechanism must preserve for every linked source contract.
    pub invariant: &'static str,
    /// Owning compiler/runtime boundary rather than a public user-facing name.
    pub owner_boundary: &'static str,
    /// Existing test or inspection anchor that must be extended by the owning slice.
    pub verification_anchor: &'static str,
    /// Why this requirement remains private when it has no direct public identity.
    pub internal_only_rationale: &'static str,
}

/// One many-to-many relation from a public `FeatureId` to a replacement compatibility feature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublicFeatureLink {
    /// Checked public `FeatureId` from the release baseline.
    pub capability_id: &'static str,
    /// Compiler-owned source-observable feature identity.
    pub feature_id: &'static str,
    /// Why this public capability contributes to the compatibility feature.
    pub rationale: &'static str,
}

/// One many-to-many relation from a compatibility feature to a private implementation requirement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FeatureRequirementLink {
    /// Compiler-owned compatibility-feature identity.
    pub feature_id: &'static str,
    /// Compiler-owned implementation-requirement identity.
    pub requirement_id: &'static str,
    /// Why the mechanism is necessary for the linked observable contract.
    pub rationale: &'static str,
}

/// Closed explanation for a baseline capability that intentionally has no direct feature relation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BaselineOutOfEnvelopeRationale {
    /// Checked public `FeatureId` that is intentionally outside the direct profile.
    pub capability_id: &'static str,
    /// Closed taxonomy category; unfinished execution is not an allowed category.
    pub category: OutOfEnvelopeCategory,
    /// Why the capability cannot become a direct Body-IR profile.
    pub rationale: String,
}

/// Closed direct-profile exclusion taxonomy for a release capability without a feature link.
///
/// This has no unfinished-implementation variant: source-observable work that remains in the envelope must be
/// represented by a planned or blocked compatibility feature with an owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum OutOfEnvelopeCategory {
    /// Package or ABI compatibility that cannot execute as a source-only Body-IR profile.
    PackageOrAbiBoundary,
    /// Host-authority service whose direct execution requires an external provider boundary.
    HostedProviderBoundary,
    /// Compiler/control-plane observability that does not itself evaluate a source function.
    CompilerControlPlane,
}

impl OutOfEnvelopeCategory {
    /// Return the stable developer-facing spelling for projections and reports.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PackageOrAbiBoundary => "PackageOrAbiBoundary",
            Self::HostedProviderBoundary => "HostedProviderBoundary",
            Self::CompilerControlPlane => "CompilerControlPlane",
        }
    }
}

/// Complete compatibility projection collected from local implementation registrations and migration scaffolding.
///
/// The aggregate is a validation and reporting view, not the owning declaration site for a language feature. New
/// durable work belongs in the relevant compiler boundary; only unresolved migration coverage may remain in the
/// explicitly marked bootstrap contributor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReplacementCompatibilityRegistry {
    /// Compiler boundaries that supplied the collected records and their migration lifecycle.
    pub registration_sources: Vec<CompatibilityRegistrationSource>,
    /// Source-observable compatibility contracts.
    pub features: Vec<CompatibilityFeature>,
    /// Private mechanisms shared by the contracts.
    pub requirements: Vec<ImplementationRequirement>,
    /// Public-capability to compatibility-feature relations.
    pub feature_links: Vec<PublicFeatureLink>,
    /// Compatibility-feature to implementation-requirement relations.
    pub requirement_links: Vec<FeatureRequirementLink>,
    /// Closed taxonomy explanations for any baseline capability without a feature relation.
    pub out_of_envelope: Vec<BaselineOutOfEnvelopeRationale>,
}

/// Named, versioned machine-readable replacement compatibility projection.
///
/// Serializing this document rather than a positional tuple makes the baseline and registry independently
/// discoverable to downstream validation or reporting tools.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReplacementCompatibilityInventoryDocument {
    /// Projection schema version, independent of the public capability-registry schema.
    pub schema_version: u32,
    /// Immutable release-pinned public baseline.
    pub baseline: PublicCapabilityBaseline,
    /// Compatibility features, requirements, relations, and contributor provenance collected from compiler modules.
    pub registry: ReplacementCompatibilityRegistry,
}

/// Validation failure in a release baseline or replacement compatibility registry.
#[derive(Debug, Error)]
#[error("replacement compatibility registry validation failed: {message}")]
pub struct RegistryValidationError {
    message: String,
}

impl RegistryValidationError {
    /// Build one validation error from deterministic individual failures.
    fn from_messages(messages: Vec<String>) -> Self {
        Self {
            message: messages.join("; "),
        }
    }
}

/// Decode the frozen v0.5 release pin and derive its complete capability baseline from checked metadata.
///
/// The committed snapshot is deliberately separate from the present-tense workspace
/// `crates/incan_stdlib/stdlib/features.incn`. Future public-registry edits therefore cannot alter or invalidate
/// this released compatibility target. Descriptor field extraction still goes through the shared checked metadata
/// path rather than a hand-maintained Rust list.
pub fn checked_v0_5_public_capability_baseline() -> Result<PublicCapabilityBaseline, RegistryValidationError> {
    let source = frozen_v0_5_capabilities_snapshot_path();
    let source_bytes = fs::read(&source).map_err(|error| {
        RegistryValidationError::from_messages(vec![format!(
            "failed to read frozen v0.5 capability snapshot {}: {error}",
            source.display()
        )])
    })?;
    if source_bytes != FROZEN_V0_5_CAPABILITIES_SOURCE {
        return Err(RegistryValidationError::from_messages(vec![format!(
            "frozen v0.5 capability snapshot {} differs from the compiled snapshot bytes",
            source.display()
        )]));
    }
    checked_v0_5_public_capability_baseline_from_source(&source)
}

/// Derive the frozen v0.5 public-capability baseline from an explicitly supplied checked source file.
///
/// This entry point is used by the projection generator and focused tests. It reads the exact file only to establish
/// the release pin; field extraction remains delegated to the compiler's checked registry metadata collector.
pub fn checked_v0_5_public_capability_baseline_from_source(
    source: &Path,
) -> Result<PublicCapabilityBaseline, RegistryValidationError> {
    let manifest = release_baseline_manifest()?;
    let source_bytes = fs::read(source).map_err(|error| {
        RegistryValidationError::from_messages(vec![format!("failed to read {}: {error}", source.display())])
    })?;
    let actual_blob = git_blob_id(&source_bytes);
    if actual_blob != manifest.release.source_blob {
        return Err(RegistryValidationError::from_messages(vec![format!(
            "{} has blob {actual_blob}, expected v0.5 pin {}",
            source.display(),
            manifest.release.source_blob
        )]));
    }
    let package = collect_checked_registry_package(source)?;
    let capabilities = public_feature_descriptors(&package)
        .map_err(|error| RegistryValidationError::from_messages(vec![error]))?
        .into_iter()
        .map(public_feature_record)
        .collect::<Vec<_>>();
    if capabilities.len() != manifest.release.expected_descriptor_count {
        return Err(RegistryValidationError::from_messages(vec![format!(
            "checked v0.5 capability count is {}, expected {}",
            capabilities.len(),
            manifest.release.expected_descriptor_count
        )]));
    }
    let ids = capabilities
        .iter()
        .map(|capability| capability.id.as_str())
        .collect::<BTreeSet<_>>();
    if ids.len() != capabilities.len() {
        return Err(RegistryValidationError::from_messages(vec![
            "checked v0.5 capability metadata contains duplicate FeatureId values".to_string(),
        ]));
    }
    Ok(PublicCapabilityBaseline {
        release: manifest.release,
        capabilities,
    })
}

/// Return the committed source snapshot path bundled with this compiler control-plane registry.
fn frozen_v0_5_capabilities_snapshot_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(FROZEN_V0_5_CAPABILITIES_PATH)
}

/// Return the collected compatibility registry for the full v0.5 public capability baseline.
///
/// The first two contributors are durable registrations beside Body IR and the bounded direct executor. The final
/// contributor is intentionally temporary migration scaffolding for contracts whose implementation boundary has not
/// landed yet, including the audited released-capability crosswalk. The collector records that distinction instead of
/// letting its bootstrap map masquerade as the permanent home for all features.
pub fn replacement_compatibility_registry() -> ReplacementCompatibilityRegistry {
    collect_replacement_compatibility_contributions(vec![
        crate::frontend::body_ir::replacement_compatibility_body_ir_contribution(),
        crate::backend::replacement::replacement_compatibility_direct_execution_contribution(),
        migration_bootstrap_compatibility_contribution(),
    ])
}

/// Join module-owned compatibility contributions into one validated projection input.
///
/// Keeping this collector small and explicit is intentional: adding a compiler boundary requires registering that
/// boundary here, but never adding a feature row to a second central catalogue.
pub(crate) fn collect_replacement_compatibility_contributions(
    contributions: Vec<ReplacementCompatibilityContribution>,
) -> ReplacementCompatibilityRegistry {
    let mut registration_sources = Vec::with_capacity(contributions.len());
    let mut features = Vec::new();
    let mut requirements = Vec::new();
    let mut feature_links = Vec::new();
    let mut requirement_links = Vec::new();
    for contribution in contributions {
        registration_sources.push(contribution.source);
        features.extend(contribution.features);
        requirements.extend(contribution.requirements);
        feature_links.extend(contribution.feature_links);
        requirement_links.extend(contribution.requirement_links);
    }
    attach_frozen_source_anchors(&mut features, &feature_links);
    ReplacementCompatibilityRegistry {
        registration_sources,
        features,
        requirements,
        feature_links,
        requirement_links,
        out_of_envelope: Vec::new(),
    }
}

/// Build a durable module-owned contribution for the collector.
pub(crate) fn local_implementation_contribution(
    id: &'static str,
    repository_path: &'static str,
    selector: &'static str,
    features: Vec<CompatibilityFeature>,
    requirements: Vec<ImplementationRequirement>,
    feature_links: Vec<PublicFeatureLink>,
    requirement_links: Vec<FeatureRequirementLink>,
) -> ReplacementCompatibilityContribution {
    contribution(
        id,
        CompatibilityRegistrationLifecycle::LocalImplementation,
        repository_path,
        selector,
        None,
        features,
        requirements,
        feature_links,
        requirement_links,
    )
}

/// Build an explicitly temporary migration contribution for records without a landed local boundary yet.
///
/// Each parameter is a distinct named field of the record being built, so grouping them into a struct would
/// only move the same list behind another type.
#[allow(clippy::too_many_arguments)]
fn migration_bootstrap_contribution(
    id: &'static str,
    repository_path: &'static str,
    selector: &'static str,
    retirement_condition: &'static str,
    features: Vec<CompatibilityFeature>,
    requirements: Vec<ImplementationRequirement>,
    feature_links: Vec<PublicFeatureLink>,
    requirement_links: Vec<FeatureRequirementLink>,
) -> ReplacementCompatibilityContribution {
    contribution(
        id,
        CompatibilityRegistrationLifecycle::MigrationBootstrap,
        repository_path,
        selector,
        Some(retirement_condition),
        features,
        requirements,
        feature_links,
        requirement_links,
    )
}

/// Construct the common contributor metadata and derive its visible record membership.
///
/// Each parameter is a distinct named field of the record being built, so grouping them into a struct would
/// only move the same list behind another type.
#[allow(clippy::too_many_arguments)]
fn contribution(
    id: &'static str,
    lifecycle: CompatibilityRegistrationLifecycle,
    repository_path: &'static str,
    selector: &'static str,
    retirement_condition: Option<&'static str>,
    features: Vec<CompatibilityFeature>,
    requirements: Vec<ImplementationRequirement>,
    feature_links: Vec<PublicFeatureLink>,
    requirement_links: Vec<FeatureRequirementLink>,
) -> ReplacementCompatibilityContribution {
    ReplacementCompatibilityContribution {
        source: CompatibilityRegistrationSource {
            id: id.to_string(),
            lifecycle,
            repository_path: repository_path.to_string(),
            selector: selector.to_string(),
            retirement_condition: retirement_condition.map(str::to_string),
            feature_ids: features.iter().map(|feature| feature.id.clone()).collect(),
            requirement_ids: requirements
                .iter()
                .map(|requirement| requirement.id.to_string())
                .collect(),
        },
        features,
        requirements,
        feature_links,
        requirement_links,
    }
}

/// Validate the baseline, records, and many-to-many relations as one control-plane unit.
///
/// A successful result means every release capability is classified, every source-observable feature has a probe and
/// appropriate ownership, every requirement is linked, and execution/comparison claims cite the required evidence.
/// It does **not** mean that replacement parity is green.
pub fn validate_replacement_compatibility_registry(
    baseline: &PublicCapabilityBaseline,
    registry: &ReplacementCompatibilityRegistry,
) -> Result<(), RegistryValidationError> {
    let mut errors = Vec::new();
    let workspace_root = match registry_workspace_root() {
        Ok(root) => Some(root),
        Err(error) => {
            errors.push(error.to_string());
            None
        }
    };
    let baseline_ids = baseline
        .capabilities
        .iter()
        .map(|capability| capability.id.as_str())
        .collect::<BTreeSet<_>>();
    if baseline_ids.len() != baseline.capabilities.len() {
        errors.push("baseline has duplicate FeatureId values".to_string());
    }
    if baseline.capabilities.len() != baseline.release.expected_descriptor_count {
        errors.push(format!(
            "baseline has {} capabilities, pin expects {}",
            baseline.capabilities.len(),
            baseline.release.expected_descriptor_count
        ));
    }
    for capability in &baseline.capabilities {
        validate_landing_provenance(capability, workspace_root.as_deref(), &mut errors);
    }

    let feature_ids = registry
        .features
        .iter()
        .map(|feature| feature.id.as_str())
        .collect::<BTreeSet<_>>();
    if feature_ids.len() != registry.features.len() {
        errors.push("compatibility features contain duplicate IDs".to_string());
    }
    let requirement_ids = registry
        .requirements
        .iter()
        .map(|requirement| requirement.id)
        .collect::<BTreeSet<_>>();
    if requirement_ids.len() != registry.requirements.len() {
        errors.push("implementation requirements contain duplicate IDs".to_string());
    }
    validate_registration_sources(
        &registry.registration_sources,
        &feature_ids,
        &requirement_ids,
        workspace_root.as_deref(),
        &mut errors,
    );

    let mapped_capabilities = registry
        .feature_links
        .iter()
        .filter(|link| baseline_ids.contains(link.capability_id))
        .map(|link| link.capability_id)
        .collect::<BTreeSet<_>>();
    let closed_out_of_envelope = registry
        .out_of_envelope
        .iter()
        .filter(|rationale| !rationale.rationale.is_empty())
        .map(|rationale| rationale.capability_id)
        .collect::<BTreeSet<_>>();
    for capability_id in &baseline_ids {
        if !mapped_capabilities.contains(capability_id) && !closed_out_of_envelope.contains(capability_id) {
            errors.push(format!(
                "baseline capability `{capability_id}` is unmapped without a closed rationale"
            ));
        }
    }
    for link in &registry.feature_links {
        if !baseline_ids.contains(link.capability_id) {
            errors.push(format!(
                "capability link names unknown baseline ID `{}`",
                link.capability_id
            ));
        }
        if !feature_ids.contains(link.feature_id) {
            errors.push(format!("capability link names unknown feature `{}`", link.feature_id));
        }
        if link.rationale.is_empty() {
            errors.push(format!(
                "capability link `{}` -> `{}` lacks a rationale",
                link.capability_id, link.feature_id
            ));
        }
    }
    for rationale in &registry.out_of_envelope {
        if !baseline_ids.contains(rationale.capability_id) {
            errors.push(format!(
                "out-of-envelope rationale names unknown baseline ID `{}`",
                rationale.capability_id
            ));
        }
        if rationale.rationale.is_empty() {
            errors.push(format!(
                "out-of-envelope rationale for `{}` lacks a closed-taxonomy explanation",
                rationale.capability_id
            ));
        }
        if mapped_capabilities.contains(rationale.capability_id) {
            errors.push(format!(
                "baseline capability `{}` is both mapped and out-of-envelope",
                rationale.capability_id
            ));
        }
    }

    let linked_features = registry
        .requirement_links
        .iter()
        .map(|link| link.feature_id)
        .collect::<BTreeSet<_>>();
    let linked_requirements = registry
        .requirement_links
        .iter()
        .map(|link| link.requirement_id)
        .collect::<BTreeSet<_>>();
    for feature in &registry.features {
        if matches!(feature.disposition, CompatibilityDisposition::Unclassified) {
            errors.push(format!("feature `{}` lacks a compatibility disposition", feature.id));
        }
        if feature.probes.is_empty() {
            errors.push(format!("feature `{}` lacks source probes", feature.id));
        }
        for probe in &feature.probes {
            validate_source_probe(feature, probe, workspace_root.as_deref(), &mut errors);
        }
        if !registry.feature_links.iter().any(|link| link.feature_id == feature.id) {
            errors.push(format!(
                "compatibility feature `{}` lacks an incoming public-capability relation",
                feature.id
            ));
        }
        if !linked_features.contains(feature.id.as_str()) {
            errors.push(format!("feature `{}` lacks implementation requirements", feature.id));
        }
        let requires_owner = !matches!(
            feature.disposition,
            CompatibilityDisposition::Preserved | CompatibilityDisposition::OutOfEnvelope
        );
        if requires_owner && feature.owner_issue.is_none() {
            errors.push(format!(
                "non-preserved in-envelope feature `{}` lacks an owning issue",
                feature.id
            ));
        }
        if requires_owner && feature.migration_or_blocker.as_deref().is_none_or(str::is_empty) {
            errors.push(format!(
                "non-preserved in-envelope feature `{}` lacks a migration or blocker note",
                feature.id
            ));
        }
        validate_feature_surface_coverage(feature, workspace_root.as_deref(), &mut errors);
        let direct_claim = matches!(
            feature.evidence.direct_replacement,
            DirectReplacementOutcome::Executable | DirectReplacementOutcome::ExplicitlyRefused
        );
        if direct_claim
            && !matches!(
                feature.evidence.surfaces.replacement_executor.status,
                EvidenceAnchorStatus::Observed
            )
        {
            errors.push(format!(
                "feature `{}` claims a direct outcome without an observed replacement-executor anchor",
                feature.id
            ));
        }
        let comparison_claim = matches!(
            feature.evidence.independent_comparison,
            IndependentComparisonState::ComparedMatch | IndependentComparisonState::ComparedMismatch
        );
        if comparison_claim {
            if !matches!(
                feature.evidence.direct_replacement,
                DirectReplacementOutcome::Executable
            ) {
                errors.push(format!(
                    "feature `{}` claims comparison without direct execution",
                    feature.id
                ));
            }
            if !matches!(feature.evidence.legacy_run, LegacyRunState::Observed) {
                errors.push(format!(
                    "feature `{}` claims comparison without a legacy run",
                    feature.id
                ));
            }
            if !matches!(
                feature.evidence.surfaces.independent_comparison,
                ComparisonEvidence::Paired { .. }
            ) {
                errors.push(format!(
                    "feature `{}` claims comparison without paired evidence",
                    feature.id
                ));
            }
        }
        if feature.evidence.is_parity_green()
            && !matches!(
                feature.evidence.surfaces.independent_comparison,
                ComparisonEvidence::Paired { .. }
            )
        {
            errors.push(format!(
                "feature `{}` is green without receipt-bound comparison evidence",
                feature.id
            ));
        }
    }
    for requirement in &registry.requirements {
        if !linked_requirements.contains(requirement.id) {
            errors.push(format!("implementation requirement `{}` is orphaned", requirement.id));
        }
        if requirement.owner_boundary.is_empty() || requirement.verification_anchor.is_empty() {
            errors.push(format!(
                "implementation requirement `{}` lacks boundary or verification anchor",
                requirement.id
            ));
        }
    }
    for link in &registry.requirement_links {
        if !feature_ids.contains(link.feature_id) {
            errors.push(format!("requirement link names unknown feature `{}`", link.feature_id));
        }
        if !requirement_ids.contains(link.requirement_id) {
            errors.push(format!(
                "requirement link names unknown requirement `{}`",
                link.requirement_id
            ));
        }
        if link.rationale.is_empty() {
            errors.push(format!(
                "requirement link `{}` -> `{}` lacks a rationale",
                link.feature_id, link.requirement_id
            ));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(RegistryValidationError::from_messages(errors))
    }
}

/// Validate that the collected projection still knows where every feature and requirement is owned.
///
/// The central collector deliberately validates provenance instead of accepting an anonymous feature vector. That
/// prevents a future edit from reintroducing a permanent hand-maintained catalogue under a different field name.
fn validate_registration_sources(
    sources: &[CompatibilityRegistrationSource],
    feature_ids: &BTreeSet<&str>,
    requirement_ids: &BTreeSet<&str>,
    workspace_root: Option<&Path>,
    errors: &mut Vec<String>,
) {
    let source_ids = sources.iter().map(|source| source.id.as_str()).collect::<BTreeSet<_>>();
    if source_ids.len() != sources.len() {
        errors.push("compatibility registration sources contain duplicate IDs".to_string());
    }
    if !sources.iter().any(|source| {
        matches!(
            source.lifecycle,
            CompatibilityRegistrationLifecycle::LocalImplementation
        )
    }) {
        errors.push("compatibility collector has no local implementation registrations".to_string());
    }

    let mut registered_feature_counts = BTreeMap::new();
    let mut registered_requirement_counts = BTreeMap::new();
    for source in sources {
        if source.id.trim().is_empty() {
            errors.push("compatibility registration source lacks an ID".to_string());
        }
        if source.repository_path.is_empty()
            || Path::new(&source.repository_path).is_absolute()
            || source.repository_path.split('/').any(|segment| segment == "..")
        {
            errors.push(format!(
                "compatibility registration source `{}` has an invalid repository-relative path",
                source.id
            ));
        }
        if source.selector.trim().is_empty() {
            errors.push(format!(
                "compatibility registration source `{}` lacks a selector",
                source.id
            ));
        }
        match source.lifecycle {
            CompatibilityRegistrationLifecycle::LocalImplementation => {
                if source.retirement_condition.is_some() {
                    errors.push(format!(
                        "local implementation registration `{}` has a migration retirement condition",
                        source.id
                    ));
                }
            }
            CompatibilityRegistrationLifecycle::MigrationBootstrap => {
                if source.retirement_condition.as_deref().is_none_or(str::is_empty) {
                    errors.push(format!(
                        "migration bootstrap registration `{}` lacks an explicit retirement condition",
                        source.id
                    ));
                }
            }
        }
        if let Some(root) = workspace_root {
            let path = root.join(&source.repository_path);
            if !path.is_file() {
                errors.push(format!(
                    "compatibility registration source `{}` points at missing repository path `{}`",
                    source.id, source.repository_path
                ));
            } else if let Ok(contents) = fs::read_to_string(&path)
                && !contents.contains(&source.selector)
            {
                errors.push(format!(
                    "compatibility registration source `{}` selector `{}` is dangling at `{}`",
                    source.id, source.selector, source.repository_path
                ));
            }
        }
        let source_feature_ids = source.feature_ids.iter().map(String::as_str).collect::<BTreeSet<_>>();
        if source_feature_ids.len() != source.feature_ids.len() {
            errors.push(format!(
                "compatibility registration source `{}` lists duplicate feature IDs",
                source.id
            ));
        }
        for feature_id in source_feature_ids {
            if !feature_ids.contains(feature_id) {
                errors.push(format!(
                    "compatibility registration source `{}` names unknown feature `{feature_id}`",
                    source.id
                ));
            }
            *registered_feature_counts.entry(feature_id).or_insert(0usize) += 1;
        }
        let source_requirement_ids = source
            .requirement_ids
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        if source_requirement_ids.len() != source.requirement_ids.len() {
            errors.push(format!(
                "compatibility registration source `{}` lists duplicate requirement IDs",
                source.id
            ));
        }
        for requirement_id in source_requirement_ids {
            if !requirement_ids.contains(requirement_id) {
                errors.push(format!(
                    "compatibility registration source `{}` names unknown requirement `{requirement_id}`",
                    source.id
                ));
            }
            *registered_requirement_counts.entry(requirement_id).or_insert(0usize) += 1;
        }
    }
    for feature_id in feature_ids {
        match registered_feature_counts.get(feature_id) {
            Some(1) => {}
            Some(count) => errors.push(format!(
                "compatibility feature `{feature_id}` is registered by {count} sources instead of exactly one"
            )),
            None => errors.push(format!(
                "compatibility feature `{feature_id}` has no registration source"
            )),
        }
    }
    for requirement_id in requirement_ids {
        match registered_requirement_counts.get(requirement_id) {
            Some(1) => {}
            Some(count) => errors.push(format!(
                "implementation requirement `{requirement_id}` is registered by {count} sources instead of exactly one"
            )),
            None => errors.push(format!(
                "implementation requirement `{requirement_id}` has no registration source"
            )),
        }
    }
}

/// Validate typed historical landing state for one frozen public capability record.
fn validate_landing_provenance(
    capability: &PublicFeatureRecord,
    workspace_root: Option<&Path>,
    errors: &mut Vec<String>,
) {
    validate_evidence_anchor(
        &format!("public capability `{}` landing provenance", capability.id),
        &capability.landing_provenance.anchor,
        EvidenceSurface::SourceAst,
        workspace_root,
        errors,
    );
    match capability.landing_provenance.state {
        LandingProvenanceState::ReleaseRegistryDeclared => {
            if capability.landing_provenance.owner_issue.is_some() || capability.landing_provenance.note.is_some() {
                errors.push(format!(
                    "public capability `{}` has release-registry provenance with unresolved owner or note",
                    capability.id
                ));
            }
        }
        LandingProvenanceState::HistoricalDiscrepancyUnresolved => {
            if capability.landing_provenance.owner_issue.is_none() {
                errors.push(format!(
                    "public capability `{}` has an unresolved historical discrepancy without an owner",
                    capability.id
                ));
            }
            if capability.landing_provenance.note.as_deref().is_none_or(str::is_empty) {
                errors.push(format!(
                    "public capability `{}` has an unresolved historical discrepancy without a note",
                    capability.id
                ));
            }
        }
    }
}

/// Validate a structured probe's positive and negative anchors.
fn validate_source_probe(
    feature: &CompatibilityFeature,
    probe: &SourceProbe,
    workspace_root: Option<&Path>,
    errors: &mut Vec<String>,
) {
    if probe.id.is_empty() {
        errors.push(format!("feature `{}` has a probe without a stable ID", feature.id));
    }
    if probe.positive.contract.is_empty() || probe.negative.contract.is_empty() {
        errors.push(format!("feature `{}` has an incomplete structured probe", feature.id));
    }
    if !matches!(probe.positive.outcome, ProbeOutcome::AcceptedBehavior)
        || !matches!(probe.negative.outcome, ProbeOutcome::IntentionalRefusal)
    {
        errors.push(format!(
            "feature `{}` probe `{}` lacks accepted/refusal outcome classifications",
            feature.id, probe.id
        ));
    }
    validate_evidence_anchor(
        &format!("feature `{}` probe `{}` positive", feature.id, probe.id),
        &probe.positive.anchor,
        EvidenceSurface::SourceAst,
        workspace_root,
        errors,
    );
    validate_evidence_anchor(
        &format!("feature `{}` probe `{}` negative", feature.id, probe.id),
        &probe.negative.anchor,
        EvidenceSurface::Typechecker,
        workspace_root,
        errors,
    );
}

/// Validate all six typed surfaces and the cross-state evidence constraints for a feature.
fn validate_feature_surface_coverage(
    feature: &CompatibilityFeature,
    workspace_root: Option<&Path>,
    errors: &mut Vec<String>,
) {
    let coverage = &feature.evidence.surfaces;
    validate_evidence_anchor(
        &format!("feature `{}` source/AST", feature.id),
        &coverage.source_ast,
        EvidenceSurface::SourceAst,
        workspace_root,
        errors,
    );
    validate_evidence_anchor(
        &format!("feature `{}` typechecker", feature.id),
        &coverage.typechecker,
        EvidenceSurface::Typechecker,
        workspace_root,
        errors,
    );
    validate_evidence_anchor(
        &format!("feature `{}` Body IR", feature.id),
        &coverage.body_ir,
        EvidenceSurface::BodyIr,
        workspace_root,
        errors,
    );
    validate_evidence_anchor(
        &format!("feature `{}` replacement executor", feature.id),
        &coverage.replacement_executor,
        EvidenceSurface::ReplacementExecutor,
        workspace_root,
        errors,
    );
    match &coverage.parity_corpus {
        ParityCorpusReference::Registered { case_ids, anchor } => {
            if case_ids.is_empty() {
                errors.push(format!("feature `{}` lacks registered #987 case IDs", feature.id));
            }
            for case_id in case_ids {
                if !registered_parity_corpus_case_id(case_id) {
                    errors.push(format!(
                        "feature `{}` names unstable or unregistered #987 case `{case_id}`",
                        feature.id
                    ));
                }
            }
            validate_evidence_anchor(
                &format!("feature `{}` registered #987 corpus", feature.id),
                anchor,
                EvidenceSurface::ParityCorpus,
                workspace_root,
                errors,
            );
            if !matches!(anchor.status, EvidenceAnchorStatus::Observed) {
                errors.push(format!(
                    "feature `{}` has registered #987 cases without an observed corpus anchor",
                    feature.id
                ));
            }
        }
        ParityCorpusReference::Planned {
            case_id,
            owner_issue,
            anchor,
        } => {
            let expected_case_id = format!("parity-987-plan-{}", feature.id);
            if *owner_issue != 987 || case_id != &expected_case_id {
                errors.push(format!(
                    "feature `{}` has an unstable #987 planned corpus reference `{case_id}`; expected `{expected_case_id}`",
                    feature.id
                ));
            }
            validate_evidence_anchor(
                &format!("feature `{}` planned #987 corpus", feature.id),
                anchor,
                EvidenceSurface::ParityCorpus,
                workspace_root,
                errors,
            );
            if !matches!(anchor.status, EvidenceAnchorStatus::Planned) || anchor.owner_issue != Some(*owner_issue) {
                errors.push(format!(
                    "feature `{}` planned #987 corpus anchor lacks #987 ownership",
                    feature.id
                ));
            }
        }
    }
    validate_comparison_evidence(
        &format!("feature `{}`", feature.id),
        feature.evidence.independent_comparison,
        &coverage.independent_comparison,
        feature.owner_issue,
        workspace_root,
        errors,
    );
    let registered_case_ids = match &coverage.parity_corpus {
        ParityCorpusReference::Registered { case_ids, .. } => Some(case_ids),
        ParityCorpusReference::Planned { .. } => None,
    };
    let mut scoped_case_ids = BTreeSet::new();
    for comparison in &coverage.scoped_comparisons {
        if !scoped_case_ids.insert(comparison.case_id.as_str()) {
            errors.push(format!(
                "feature `{}` has duplicate scoped comparison case `{}`",
                feature.id, comparison.case_id
            ));
        }
        if !registered_parity_corpus_case_id(&comparison.case_id)
            || !registered_case_ids.is_some_and(|case_ids| case_ids.contains(&comparison.case_id))
        {
            errors.push(format!(
                "feature `{}` has scoped comparison for unlinked #987 case `{}`",
                feature.id, comparison.case_id
            ));
        }
        if matches!(&comparison.evidence, ComparisonEvidence::Paired { .. })
            && matches!(
                feature.evidence.direct_replacement,
                DirectReplacementOutcome::ExplicitlyRefused | DirectReplacementOutcome::OutsideDirectExecution
            )
        {
            errors.push(format!(
                "feature `{}` has paired scoped case `{}` despite excluding direct execution",
                feature.id, comparison.case_id
            ));
        }
        validate_comparison_evidence(
            &format!("feature `{}` case `{}`", feature.id, comparison.case_id),
            comparison.state,
            &comparison.evidence,
            feature.owner_issue,
            workspace_root,
            errors,
        );
    }
}

/// Validate one aggregate or corpus-case comparison fact without widening its scope.
fn validate_comparison_evidence(
    label: &str,
    state: IndependentComparisonState,
    evidence: &ComparisonEvidence,
    feature_owner_issue: Option<u32>,
    workspace_root: Option<&Path>,
    errors: &mut Vec<String>,
) {
    match evidence {
        ComparisonEvidence::Unavailable {
            comparison_infrastructure,
            outstanding_evidence,
        } => {
            if !matches!(state, IndependentComparisonState::NonGreenShadowUnavailable) {
                errors.push(format!("{label} has an invalid unavailable-comparison classification"));
            }
            validate_evidence_anchor(
                &format!("{label} completed comparison infrastructure"),
                &comparison_infrastructure.anchor,
                EvidenceSurface::IndependentComparison,
                workspace_root,
                errors,
            );
            if comparison_infrastructure.issue != COMPLETED_COMPARISON_INFRASTRUCTURE_ISSUE {
                errors.push(format!(
                    "{label} records completed comparison infrastructure #{}, expected #{}",
                    comparison_infrastructure.issue, COMPLETED_COMPARISON_INFRASTRUCTURE_ISSUE
                ));
            }
            if !matches!(comparison_infrastructure.anchor.status, EvidenceAnchorStatus::Observed) {
                errors.push(format!(
                    "{label} has unavailable comparison evidence without an observed completed-infrastructure anchor"
                ));
            }
            match outstanding_evidence {
                OutstandingComparisonEvidence::Scheduled { owner_issue, note } => {
                    if *owner_issue == COMPLETED_COMPARISON_INFRASTRUCTURE_ISSUE {
                        errors.push(format!(
                            "{label} assigns completed comparison infrastructure #{} as outstanding evidence owner",
                            COMPLETED_COMPARISON_INFRASTRUCTURE_ISSUE
                        ));
                    }
                    if Some(*owner_issue) != feature_owner_issue {
                        errors.push(format!(
                            "{label} schedules outstanding comparison evidence for #{owner_issue}, which does not match the feature owner"
                        ));
                    }
                    if note.trim().is_empty() {
                        errors.push(format!(
                            "{label} has a scheduled comparison owner without an evidence note"
                        ));
                    }
                }
                OutstandingComparisonEvidence::UnscheduledDebt { note } => {
                    if let Some(owner_issue) = feature_owner_issue {
                        errors.push(format!(
                            "{label} has unscheduled comparison evidence debt despite feature owner #{owner_issue}"
                        ));
                    }
                    if note.trim().is_empty() {
                        errors.push(format!(
                            "{label} has unscheduled comparison evidence debt without a note"
                        ));
                    }
                }
            }
        }
        ComparisonEvidence::Paired {
            legacy_receipt,
            replacement_receipt,
            comparison_record,
        } => {
            if !matches!(
                state,
                IndependentComparisonState::ComparedMatch | IndependentComparisonState::ComparedMismatch
            ) {
                errors.push(format!(
                    "{label} has paired comparison evidence without a compared state"
                ));
            }
            for (receipt_label, anchor) in [
                ("legacy receipt", legacy_receipt),
                ("replacement receipt", replacement_receipt),
                ("comparison record", comparison_record),
            ] {
                validate_evidence_anchor(
                    &format!("{label} {receipt_label}"),
                    anchor,
                    EvidenceSurface::IndependentComparison,
                    workspace_root,
                    errors,
                );
                if !matches!(anchor.status, EvidenceAnchorStatus::Observed) {
                    errors.push(format!(
                        "{label} has a paired comparison with a non-observed {receipt_label}"
                    ));
                }
            }
        }
    }
}

/// Validate an anchor's typed surface, ownership, relative path, and observed selector.
fn validate_evidence_anchor(
    label: &str,
    anchor: &EvidenceAnchor,
    expected_surface: EvidenceSurface,
    workspace_root: Option<&Path>,
    errors: &mut Vec<String>,
) {
    if anchor.surface != expected_surface {
        errors.push(format!(
            "{label} has surface {}, expected {}",
            anchor.surface.as_str(),
            expected_surface.as_str()
        ));
    }
    if anchor.repository_path.is_empty()
        || Path::new(&anchor.repository_path).is_absolute()
        || anchor.repository_path.split('/').any(|segment| segment == "..")
    {
        errors.push(format!("{label} has an invalid repository-relative path"));
    }
    if anchor.selector.is_empty() || anchor.note.is_empty() {
        errors.push(format!("{label} lacks a selector or explanation"));
    }
    match anchor.status {
        EvidenceAnchorStatus::Observed => {
            if anchor.owner_issue.is_some() {
                errors.push(format!("{label} is observed but names a planned-work owner"));
            }
            validate_observed_anchor_location(label, anchor, workspace_root, errors);
        }
        EvidenceAnchorStatus::Planned => {
            if anchor.owner_issue.is_none() {
                errors.push(format!("{label} is planned without an owning issue"));
            }
            validate_observed_anchor_location(label, anchor, workspace_root, errors);
        }
        EvidenceAnchorStatus::NotApplicable => {
            if anchor.owner_issue.is_some() {
                errors.push(format!("{label} is not applicable but names a planned-work owner"));
            }
        }
    }
}

/// Require that an anchor's repository path exists when it is observed or planned.
fn validate_anchor_path(label: &str, anchor: &EvidenceAnchor, workspace_root: Option<&Path>, errors: &mut Vec<String>) {
    let Some(root) = workspace_root else {
        return;
    };
    if !root.join(&anchor.repository_path).is_file() {
        errors.push(format!(
            "{label} points at missing repository path `{}`",
            anchor.repository_path
        ));
    }
}

/// Require that an observed anchor's selector still exists at its recorded repository path.
fn validate_observed_anchor_location(
    label: &str,
    anchor: &EvidenceAnchor,
    workspace_root: Option<&Path>,
    errors: &mut Vec<String>,
) {
    validate_anchor_path(label, anchor, workspace_root, errors);
    let Some(root) = workspace_root else {
        return;
    };
    let path = root.join(&anchor.repository_path);
    if !anchor_selector_resolves(&path, &anchor.selector) {
        errors.push(format!(
            "{label} selector `{}` is dangling at `{}`",
            anchor.selector, anchor.repository_path
        ));
    }
}

/// Return whether a selector still resolves for one recorded module, including inside that module's own submodules.
///
/// A recorded path names a *module*, not a physical file that must never move. `foo.rs` and `foo/` are one module in
/// Rust, so a selector that moved from `body_ir.rs` into `body_ir/match_.rs` has not gone dangling — it is still on
/// the same surface the anchor is recording. Searching the module's own directory keeps the anchor pinned to that
/// surface while letting a module be split without rewriting every registration. The search deliberately does not
/// recurse beyond the module's own tree, so a selector that genuinely leaves the surface still reports as dangling.
fn anchor_selector_resolves(module_path: &Path, selector: &str) -> bool {
    if fs::read_to_string(module_path).is_ok_and(|contents| contents.contains(selector)) {
        return true;
    }
    let Some(module_dir) = module_path
        .file_stem()
        .map(|stem| module_path.with_file_name(stem))
        .filter(|dir| dir.is_dir())
    else {
        return false;
    };
    let Ok(entries) = fs::read_dir(&module_dir) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        entry.path().extension().is_some_and(|ext| ext == "rs")
            && fs::read_to_string(entry.path()).is_ok_and(|contents| contents.contains(selector))
    })
}

/// Return whether an existing #987 case ID is part of the reviewed stable seed corpus.
fn registered_parity_corpus_case_id(case_id: &str) -> bool {
    matches!(
        case_id,
        "replacement-body-v0-001"
            | "replacement-body-v0-002"
            | "replacement-body-v0-003"
            | "replacement-body-v0-004"
            | "replacement-body-v0-005"
            | "replacement-body-v0-006"
            | "replacement-body-v0-007"
            | "replacement-body-v0-018"
            | "replacement-body-v0-019"
            | "replacement-body-v0-020"
            | "replacement-body-v0-021"
            | "replacement-body-v0-022"
            | "replacement-body-v0-023"
            | "replacement-body-v0-024"
            | "replacement-body-v0-025"
            | "replacement-body-v0-026"
            | "replacement-body-v0-027"
            | "replacement-body-v0-028"
            | "replacement-body-v0-030"
            | "replacement-body-v0-029"
    )
}

/// Render a deterministic developer-facing joined projection from the baseline and compiler-owned relations.
///
/// The projection is intentionally a report rather than an authority. The checked public registry, compatibility
/// records, and requirement records above remain the inputs. A matched corpus case remains scoped to that case;
/// an incomplete feature stays visibly non-green until its whole source contract has paired evidence.
pub fn render_developer_projection(
    baseline: &PublicCapabilityBaseline,
    registry: &ReplacementCompatibilityRegistry,
) -> Result<String, RegistryValidationError> {
    validate_replacement_compatibility_registry(baseline, registry)?;
    let mut output = String::new();
    output.push_str("# Replacement compatibility inventory\n\n");
    output.push_str("!!! warning \"Generated control-plane reference\"\n\n");
    output.push_str("    Do not edit this page by hand. Regenerate it from the checked public-capability baseline and compiler-boundary registrations.\n\n");
    output.push_str("This is a validated migration control plane, not a permanent second language-feature catalogue and not a parity claim. Durable feature and private-mechanism records are registered beside the compiler boundary that owns them; the collector joins and validates them here. The explicitly marked migration bootstrap exists only while unlanded work lacks such a boundary. A feature row turns green only after direct execution and an independent, receipt-bound source-observable comparison for its full contract. A matched corpus case remains scoped evidence and cannot promote an incomplete feature. Generated Rust, Body IR representation, and legacy compilation are separate facts.\n\n");
    output.push_str("## Release-pinned public baseline\n\n");
    output.push_str(&format!(
        "- Release: `{}` at `{}`\n- Baseline role: `{}`\n- Checked source blob: `{}`\n- Capability descriptors: `{}`\n- Retirement: {}\n\n",
        baseline.release.tag,
        baseline.release.revision,
        baseline.release.role.as_str(),
        baseline.release.source_blob,
        baseline.capabilities.len(),
        baseline.release.retirement_condition,
    ));
    output.push_str("The `v0.5.0` source is a frozen migration baseline, not the beginning of a version archive. It is retained only under the stated retirement condition.\n\n");
    output.push_str("## Collector assembly and bootstrap retirement\n\n");
    output
        .push_str("| Contributor | Lifecycle | Features | Private requirements | Location | Retirement condition |\n");
    output.push_str("|---|---|---|---|---|---|\n");
    for source in sorted_registration_sources(&registry.registration_sources) {
        let retirement_condition = source.retirement_condition.as_deref().unwrap_or("-");
        output.push_str(&format!(
            "| `{}` | {} | {} | {} | `{}::{}` | {} |\n",
            source.id,
            source.lifecycle.as_str(),
            source.feature_ids.len(),
            source.requirement_ids.len(),
            source.repository_path,
            source.selector,
            retirement_condition,
        ));
    }
    output.push('\n');
    output.push_str("## Compatibility features\n\n");
    output.push_str("| Feature | Source contract | Legacy run | Body IR | Direct replacement | #987 | Feature comparison | Scoped case comparisons | Disposition | Owner |\n");
    output.push_str("|---|---|---|---|---|---|---|---|---|---|\n");
    for feature in sorted_features(&registry.features) {
        let owner = feature
            .owner_issue
            .map(|issue| format!("#{issue}"))
            .unwrap_or_else(|| "-".to_string());
        output.push_str(&format!(
            "| `{}` | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            feature.id,
            feature.evidence.source_contract.as_str(),
            feature.evidence.legacy_run.as_str(),
            feature.evidence.body_ir.as_str(),
            feature.evidence.direct_replacement.as_str(),
            parity_corpus_reference_label(&feature.evidence.surfaces.parity_corpus),
            feature.evidence.independent_comparison.as_str(),
            scoped_comparison_label(&feature.evidence.surfaces.scoped_comparisons),
            feature.disposition.as_str(),
            owner
        ));
    }
    output.push_str("\n## Public capability crosswalk\n\n");
    output.push_str(
        "| Capability | Since | RFC | Landing provenance | Compatibility features |\n|---|---:|---|---|---|\n",
    );
    let links_by_capability = links_by_capability(&registry.feature_links);
    for capability in &baseline.capabilities {
        let features = links_by_capability
            .get(capability.id.as_str())
            .map(|links| {
                links
                    .iter()
                    .map(|link| format!("`{}`", link.feature_id))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_else(|| "closed out-of-envelope rationale".to_string());
        output.push_str(&format!(
            "| `{}` | {} | {} | {} | {} |\n",
            capability.id,
            capability.since,
            capability.rfc,
            landing_provenance_label(&capability.landing_provenance),
            features
        ));
    }
    output.push_str("\n## Private implementation requirements\n\n");
    output.push_str("| Requirement | Owning boundary | Enabled features | Verification anchor |\n|---|---|---|---|\n");
    let links_by_requirement = links_by_requirement(&registry.requirement_links);
    for requirement in sorted_requirements(&registry.requirements) {
        let features = links_by_requirement
            .get(requirement.id)
            .map(|links| {
                links
                    .iter()
                    .map(|link| format!("`{}`", link.feature_id))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_else(|| "(invalid: orphaned)".to_string());
        output.push_str(&format!(
            "| `{}` | {} | {} | {} |\n",
            requirement.id, requirement.owner_boundary, features, requirement.verification_anchor
        ));
    }
    output.push_str("\n## Remaining-work issue map\n\n");
    output.push_str("Every planned feature below has a currently open mechanism owner. #1146 is completed comparison infrastructure: it supplies reusable provenance, never ownership of missing comparison evidence. Scheduled evidence belongs to its feature/runtime owner; direct profiles without one carry explicit unscheduled evidence debt. Stable corpus rows `replacement-body-v0-001` and `replacement-body-v0-020` through `replacement-body-v0-030` have case-scoped paired matches, while all incomplete features and uncovered cases remain non-green.\n\n");
    let features_by_owner = features_by_owner(&registry.features);
    for (owner, features) in features_by_owner {
        output.push_str(&format!(
            "- #{}: {}\n",
            owner,
            features
                .iter()
                .map(|feature| format!("`{}`", feature.id))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    output.push_str("\n## Probe and ownership obligations\n\n");
    for feature in sorted_features(&registry.features) {
        output.push_str(&format!("### `{}`\n\n{}\n\n", feature.id, feature.contract));
        for probe in &feature.probes {
            output.push_str(&format!(
                "- `{}` — positive {} at {}; negative {} at {}\n",
                probe.id,
                probe.positive.outcome.as_str(),
                render_evidence_anchor(&probe.positive.anchor),
                probe.negative.outcome.as_str(),
                render_evidence_anchor(&probe.negative.anchor),
            ));
            output.push_str(&format!(
                "  - Positive contract: {}\n  - Negative contract: {}\n",
                probe.positive.contract, probe.negative.contract
            ));
            output.push_str(&format!(
                "- Source/AST: {}\n- Typechecker: {}\n- Body IR: {}\n- Replacement executor: {}\n- Aggregate comparison: {}\n",
                render_evidence_anchor(&feature.evidence.surfaces.source_ast),
                render_evidence_anchor(&feature.evidence.surfaces.typechecker),
                render_evidence_anchor(&feature.evidence.surfaces.body_ir),
                render_evidence_anchor(&feature.evidence.surfaces.replacement_executor),
                comparison_evidence_label(&feature.evidence.surfaces.independent_comparison)
            ));
            for comparison in &feature.evidence.surfaces.scoped_comparisons {
                output.push_str(&format!(
                    "- Case `{}` ({}) using completed comparison infrastructure #1146: {}\n",
                    comparison.case_id,
                    comparison.state.as_str(),
                    comparison_evidence_label(&comparison.evidence),
                ));
            }
        }
        if let Some(note) = &feature.migration_or_blocker {
            output.push_str(&format!("- Blocker/migration: {}\n", note));
        }
        output.push('\n');
    }
    if output.ends_with("\n\n") {
        output.pop();
    }
    Ok(output)
}

/// Render the baseline and registry as a deterministic machine-readable JSON document.
pub fn render_machine_readable_inventory(
    baseline: &PublicCapabilityBaseline,
    registry: &ReplacementCompatibilityRegistry,
) -> Result<String, RegistryValidationError> {
    validate_replacement_compatibility_registry(baseline, registry)?;
    serde_json::to_string_pretty(&ReplacementCompatibilityInventoryDocument {
        schema_version: REPLACEMENT_COMPATIBILITY_INVENTORY_SCHEMA_VERSION,
        baseline: baseline.clone(),
        registry: registry.clone(),
    })
    .map_err(|error| RegistryValidationError::from_messages(vec![format!("failed to serialize inventory: {error}")]))
}

/// Write the readable projection and its machine-readable companion from the same validated records.
pub fn write_replacement_compatibility_inventory(
    markdown_path: &Path,
    json_path: &Path,
) -> Result<(), RegistryValidationError> {
    let baseline = checked_v0_5_public_capability_baseline()?;
    let registry = replacement_compatibility_registry();
    let markdown = render_developer_projection(&baseline, &registry)?;
    let json = render_machine_readable_inventory(&baseline, &registry)?;
    fs::write(markdown_path, markdown).map_err(|error| {
        RegistryValidationError::from_messages(vec![format!("failed to write {}: {error}", markdown_path.display())])
    })?;
    fs::write(json_path, json).map_err(|error| {
        RegistryValidationError::from_messages(vec![format!("failed to write {}: {error}", json_path.display())])
    })
}

/// Parse the committed release pin that makes the 0.5 target explicit.
fn release_baseline_manifest() -> Result<BaselineManifest, RegistryValidationError> {
    let manifest: BaselineManifest = serde_json::from_str(BASELINE_MANIFEST).map_err(|error| {
        RegistryValidationError::from_messages(vec![format!("invalid v0.5 baseline manifest: {error}")])
    })?;
    if manifest.release.source_snapshot_path != FROZEN_V0_5_CAPABILITIES_PATH {
        return Err(RegistryValidationError::from_messages(vec![format!(
            "v0.5 baseline manifest points at `{}`, expected committed snapshot `{FROZEN_V0_5_CAPABILITIES_PATH}`",
            manifest.release.source_snapshot_path
        )]));
    }
    if manifest.release.retirement_condition.trim().is_empty() {
        return Err(RegistryValidationError::from_messages(vec![
            "v0.5 migration baseline lacks an explicit retirement condition".to_string(),
        ]));
    }
    Ok(manifest)
}

/// Collect checked registry metadata through the established CLI/session inspection boundary.
///
/// The compatibility registry depends only on the resulting checked metadata shape. Source/module collection remains
/// owned by the existing inspection path so this control plane does not grow a second parser or project resolver.
fn collect_checked_registry_package(
    source: &Path,
) -> Result<crate::frontend::registry_metadata::CheckedRegistryMetadataPackage, RegistryValidationError> {
    #[cfg(feature = "cli")]
    {
        crate::cli::commands::tools::collect_registry_metadata_package(source)
            .map_err(|error| RegistryValidationError::from_messages(vec![error.to_string()]))
    }
    #[cfg(not(feature = "cli"))]
    {
        let _ = source;
        Err(RegistryValidationError::from_messages(vec![
            "checked public-capability collection requires the compiler CLI feature".to_string(),
        ]))
    }
}

/// Locate the checkout whose code and test anchors the registry is allowed to inspect at runtime.
///
/// Test and generator binaries can share a target directory across worktrees, so this must prefer an explicit source
/// root or the process working directory over the compile-time manifest path. The fallback is only for installed or
/// archived generator use.
fn registry_workspace_root() -> Result<PathBuf, RegistryValidationError> {
    let current_dir = std::env::current_dir().map_err(|error| {
        RegistryValidationError::from_messages(vec![format!("failed to resolve current directory: {error}")])
    })?;
    if let Some(root) = workspace_ancestor(&current_dir) {
        return Ok(root);
    }
    if let Some(root) = std::env::var_os("INCAN_SOURCE_ROOT")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .filter(|root| root.join("Cargo.toml").is_file() && root.join(LIVE_FEATURES_SOURCE).is_file())
    {
        return Ok(root);
    }
    let manifest_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if manifest_root.join("Cargo.toml").is_file() && manifest_root.join(LIVE_FEATURES_SOURCE).is_file() {
        return Ok(manifest_root);
    }
    Err(RegistryValidationError::from_messages(vec![format!(
        "could not locate a checkout containing {LIVE_FEATURES_SOURCE} from {}",
        current_dir.display()
    )]))
}

/// Return the nearest workspace root above a process path when it owns the public capability source.
fn workspace_ancestor(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|candidate| candidate.join("Cargo.toml").is_file() && candidate.join(LIVE_FEATURES_SOURCE).is_file())
        .map(Path::to_path_buf)
}

/// Compute a Git blob ID from source bytes so the baseline pin does not rely on an ambient checkout command.
fn git_blob_id(source: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(format!("blob {}\0", source.len()).as_bytes());
    hasher.update(source);
    hex::encode(hasher.finalize())
}

/// Convert the shared checked-metadata decoder's private projection to the public baseline record.
fn public_feature_record(descriptor: PublicFeatureDescriptor) -> PublicFeatureRecord {
    let id = descriptor.id;
    PublicFeatureRecord {
        landing_provenance: landing_provenance_for(&id),
        id,
        name: descriptor.name,
        category: descriptor.category,
        since: descriptor.since,
        rfc: descriptor.rfc,
        stability: descriptor.stability,
        activation: descriptor.activation,
        summary: descriptor.summary,
        canonical_forms: descriptor.canonical_forms,
        prefer_over: descriptor.prefer_over,
        references: descriptor.references,
    }
}

/// Return the explicit historical provenance state for one release capability descriptor.
fn landing_provenance_for(capability_id: &str) -> LandingProvenance {
    let (state, owner_issue, note) = match capability_id {
        "CodegraphInspection" => (
            LandingProvenanceState::HistoricalDiscrepancyUnresolved,
            Some(1153),
            Some(
                "The release descriptor is linked, but the original landing trail needs reconciliation before an RFC or merge claim is made.",
            ),
        ),
        "TypeTokensReflection" => (
            LandingProvenanceState::HistoricalDiscrepancyUnresolved,
            Some(1153),
            Some(
                "The release descriptor records the public contract, while the historical landing trail remains unresolved.",
            ),
        ),
        "ValueEnums" => (
            LandingProvenanceState::HistoricalDiscrepancyUnresolved,
            Some(1153),
            Some(
                "The release descriptor records the public contract, while the historical landing trail remains unresolved.",
            ),
        ),
        "AsyncAwait" => (
            LandingProvenanceState::HistoricalDiscrepancyUnresolved,
            Some(1153),
            Some(
                "RFC 023 is the declared provenance, but a matching shipped-marker and landing trail still need reconciliation.",
            ),
        ),
        "StdWeb" => (
            LandingProvenanceState::HistoricalDiscrepancyUnresolved,
            Some(1153),
            Some(
                "RFC 023 is the declared provenance, but a matching shipped-marker and landing trail still need reconciliation.",
            ),
        ),
        _ => (LandingProvenanceState::ReleaseRegistryDeclared, None, None),
    };
    LandingProvenance {
        state,
        anchor: observed_anchor(
            EvidenceSurface::SourceAst,
            FROZEN_V0_5_CAPABILITIES_PATH,
            capability_id,
            "The frozen release-era descriptor is the audited public-contract source.",
        ),
        owner_issue,
        note: note.map(str::to_string),
    }
}

/// Attach a concrete frozen public-source anchor to each compatibility feature through its first checked relation.
fn attach_frozen_source_anchors(features: &mut [CompatibilityFeature], links: &[PublicFeatureLink]) {
    for feature in features {
        let Some(link) = links.iter().find(|link| link.feature_id == feature.id) else {
            continue;
        };
        feature.evidence.surfaces.source_ast = observed_anchor(
            EvidenceSurface::SourceAst,
            FROZEN_V0_5_CAPABILITIES_PATH,
            link.capability_id,
            "The linked frozen public descriptor supplies the source-level contract crosswalk.",
        );
        feature.probes[0].positive.anchor = feature.evidence.surfaces.source_ast.clone();
    }
}

/// Build an observed repository anchor with no deferred implementation owner.
fn observed_anchor(surface: EvidenceSurface, repository_path: &str, selector: &str, note: &str) -> EvidenceAnchor {
    EvidenceAnchor {
        surface,
        status: EvidenceAnchorStatus::Observed,
        repository_path: repository_path.to_string(),
        selector: selector.to_string(),
        owner_issue: None,
        note: note.to_string(),
    }
}

/// Build a planned repository anchor with an explicit implementation owner.
fn planned_anchor(
    surface: EvidenceSurface,
    repository_path: &str,
    selector: &str,
    owner_issue: u32,
    note: &str,
) -> EvidenceAnchor {
    EvidenceAnchor {
        surface,
        status: EvidenceAnchorStatus::Planned,
        repository_path: repository_path.to_string(),
        selector: selector.to_string(),
        owner_issue: Some(owner_issue),
        note: note.to_string(),
    }
}

/// Sort compatibility features by stable identity for deterministic developer output.
fn sorted_features(features: &[CompatibilityFeature]) -> Vec<&CompatibilityFeature> {
    let mut sorted = features.iter().collect::<Vec<_>>();
    sorted.sort_by_key(|feature| feature.id.as_str());
    sorted
}

/// Sort contributor provenance by stable identity for deterministic developer output.
fn sorted_registration_sources(sources: &[CompatibilityRegistrationSource]) -> Vec<&CompatibilityRegistrationSource> {
    let mut sorted = sources.iter().collect::<Vec<_>>();
    sorted.sort_by_key(|source| source.id.as_str());
    sorted
}

/// Sort implementation requirements by stable identity for deterministic developer output.
fn sorted_requirements(requirements: &[ImplementationRequirement]) -> Vec<&ImplementationRequirement> {
    let mut sorted = requirements.iter().collect::<Vec<_>>();
    sorted.sort_by_key(|requirement| requirement.id);
    sorted
}

/// Render one typed anchor in the readable developer projection.
fn render_evidence_anchor(anchor: &EvidenceAnchor) -> String {
    let owner = anchor
        .owner_issue
        .map(|issue| format!("; owner #{issue}"))
        .unwrap_or_default();
    format!(
        "{} `{}::{}`{}",
        anchor.status.as_str(),
        anchor.repository_path,
        anchor.selector,
        owner
    )
}

/// Render the stable #987 relationship without reducing it to a prose-only probe.
fn parity_corpus_reference_label(reference: &ParityCorpusReference) -> String {
    match reference {
        ParityCorpusReference::Registered { case_ids, .. } => format!("registered {}", case_ids.join(", ")),
        ParityCorpusReference::Planned { case_id, .. } => format!("planned {case_id}"),
    }
}

/// Render case-scoped comparison outcomes without implying feature-wide completion.
fn scoped_comparison_label(comparisons: &[CorpusCaseComparisonEvidence]) -> String {
    if comparisons.is_empty() {
        return "-".to_string();
    }
    comparisons
        .iter()
        .map(|comparison| format!("{}: {}", comparison.case_id, comparison.state.as_str()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Render factored comparison evidence while preserving its unavailable or paired state.
fn comparison_evidence_label(evidence: &ComparisonEvidence) -> String {
    match evidence {
        ComparisonEvidence::Unavailable {
            comparison_infrastructure,
            outstanding_evidence,
        } => {
            format!(
                "unavailable; completed comparison infrastructure #{} at {}; {}",
                comparison_infrastructure.issue,
                render_evidence_anchor(&comparison_infrastructure.anchor),
                outstanding_comparison_evidence_label(outstanding_evidence),
            )
        }
        ComparisonEvidence::Paired {
            legacy_receipt,
            replacement_receipt,
            comparison_record,
        } => format!(
            "paired {}; {}; {}",
            render_evidence_anchor(legacy_receipt),
            render_evidence_anchor(replacement_receipt),
            render_evidence_anchor(comparison_record)
        ),
    }
}

/// Render the owner or explicit debt for comparison evidence without reassigning completed infrastructure work.
fn outstanding_comparison_evidence_label(evidence: &OutstandingComparisonEvidence) -> String {
    match evidence {
        OutstandingComparisonEvidence::Scheduled { owner_issue, note } => {
            format!("outstanding evidence owner #{owner_issue}: {note}")
        }
        OutstandingComparisonEvidence::UnscheduledDebt { note } => {
            format!("unscheduled evidence debt: {note}")
        }
    }
}

/// Render release provenance state and its explicit historical-discovery owner when applicable.
fn landing_provenance_label(provenance: &LandingProvenance) -> String {
    let owner = provenance
        .owner_issue
        .map(|issue| format!("; owner #{issue}"))
        .unwrap_or_default();
    format!("{}{}", provenance.state.as_str(), owner)
}

/// Group public-capability links by their checked baseline identity.
fn links_by_capability(links: &[PublicFeatureLink]) -> BTreeMap<&str, Vec<&PublicFeatureLink>> {
    let mut grouped = BTreeMap::new();
    for link in links {
        grouped.entry(link.capability_id).or_insert_with(Vec::new).push(link);
    }
    for links in grouped.values_mut() {
        links.sort_by_key(|link| link.feature_id);
    }
    grouped
}

/// Group requirement links by their private requirement identity.
fn links_by_requirement(links: &[FeatureRequirementLink]) -> BTreeMap<&str, Vec<&FeatureRequirementLink>> {
    let mut grouped = BTreeMap::new();
    for link in links {
        grouped.entry(link.requirement_id).or_insert_with(Vec::new).push(link);
    }
    for links in grouped.values_mut() {
        links.sort_by_key(|link| link.feature_id);
    }
    grouped
}

/// Group planned and blocked features by their declared implementation issue owner.
fn features_by_owner(features: &[CompatibilityFeature]) -> BTreeMap<u32, Vec<&CompatibilityFeature>> {
    let mut grouped = BTreeMap::new();
    for feature in features {
        if let Some(owner) = feature.owner_issue {
            grouped.entry(owner).or_insert_with(Vec::new).push(feature);
        }
    }
    for features in grouped.values_mut() {
        features.sort_by_key(|feature| feature.id.as_str());
    }
    grouped
}

/// One deserializable baseline manifest whose descriptor data remains in checked metadata.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct BaselineManifest {
    release: ReleasePin,
}

/// Construct temporary migration coverage for feature contracts without a landed local registration yet.
///
/// This is deliberately not the permanent feature registry. As a coherent mechanism lands, its record must move to
/// the owning module's contribution and disappear from this function. The collector validates that the bootstrap
/// source remains visibly marked until that transition is complete.
fn migration_bootstrap_compatibility_features() -> Vec<CompatibilityFeature> {
    vec![
        planned_feature(
            "call.named-and-variadic",
            "Named calls preserve resolved targets, generic arguments, positional/named binding, and spread diagnostics.",
            988,
            "Closed #1152 delivered the callable runtime substrate; open #988 owns broadening named, variadic, and spread execution with receipt-bound evidence.",
        ),
        planned_feature(
            "decorators.dsl-surfaces",
            "Decorators and scoped DSL surfaces preserve activation, dispatch, and source-owned diagnostics.",
            555,
            "Surface packs and decorators require a source-to-runtime dispatch boundary before direct execution can classify them.",
        ),
        planned_feature(
            "diagnostics.stable",
            "Source diagnostics retain intentional acceptance/refusal boundaries, spans, and machine-readable identity.",
            655,
            "The compatibility report and corpus need receipt-bound diagnostic evidence; generated Rust diagnostics are not a substitute.",
        ),
        planned_feature(
            "error.result-and-try",
            "Result combinators and explicit propagation retain success, error, ordering, and diagnostic behavior.",
            988,
            "Closed #1101 delivered the Body IR vocabulary and closed #1154 delivered Result/error value routing; open #988 owns broadening and comparing the remaining execution profile.",
        ),
        planned_feature(
            "generator.expressions",
            "Generator expressions preserve construction-versus-consumption timing and lazy collection in the admitted profile.",
            988,
            "Closed #1152 delivered the bounded generator-expression collect path; open #988 owns broader consumption and comparison, which remain non-green.",
        ),
        planned_feature(
            "generator.functions",
            "Generator functions suspend and resume without replaying prior effects or losing local state.",
            988,
            "Closed #1152 delivered the callable/lazy-generator substrate; open #988 owns the generator-function frames and resumption forms that remain explicit replacement refusals.",
        ),
        planned_feature_with_bounded_cases(
            "iteration.protocol-and-adapters",
            "Iterator protocols, adapters, and consumers preserve lazy dispatch, callback timing, exhaustion, and errors.",
            988,
            "Closed #1152 delivered the first callable/lazy-generator adapter profile; open #988 owns broader protocol dispatch, which remains blocked.",
        ),
        planned_feature(
            "iteration.user-and-fallible",
            "User-defined and fallible iteration preserve protocol calls, terminal behavior, and error routing.",
            988,
            "Closed #1101 delivered the Body IR protocol vocabulary; open #988 owns the runtime dispatch and error-routing profile required to admit these forms.",
        ),
        planned_feature_with_bounded_cases(
            "language.aggregates-and-projections",
            "Tuple, list, dict, set, slice, projection, mutation, equality, and ordering retain source semantics.",
            988,
            "Source-local scalar-key set/dict membership and entry count plus nonempty integer-list sorting execute directly. Standalone replacement-body-v0-020, replacement-body-v0-026 and replacement-body-v0-028 prove their exact streams and typed results across independent routes. These bounded proofs do not establish the full aggregate or ordering contract. Closed #1154 delivered the direct value-state substrate; open #988 owns broadening storage, projection, mutation, equality, and ordering execution.",
        ),
        planned_feature(
            "language.control-flow-complete",
            "Control flow beyond the bounded scalar profile preserves value-carrying branches, pattern binding, loop results, and diagnostics.",
            988,
            "The current direct profile covers only the bounded scalar subset. Closed #1154 delivered the value and pattern runtime substrate; open #988 owns the remaining control-flow execution and comparison profile.",
        ),
        planned_feature(
            "language.match-and-patterns",
            "Match, destructuring, alternation, guards, and exhaustiveness preserve branch selection and diagnostics.",
            988,
            "Closed #1101 delivered the Body IR vocabulary and closed #1154 delivered pattern dispatch over direct values; open #988 owns broadening and comparing the remaining match surface.",
        ),
        planned_feature_with_bounded_cases(
            "language.strings-and-format",
            "String operators and formatting preserve interpolation order, conversions, and runtime failures.",
            988,
            "String concatenation, bounded scalar interpolation, selected canonical string helpers and Unicode-scalar string length execute directly. Closed #1101 delivered the Body IR vocabulary. The separate replacement-body-v0-021 and replacement-body-v0-024 corpus cases prove those bounded profiles, not this full formatting contract; open #988 owns broader execution and feature parity remains non-green.",
        ),
        planned_feature(
            "module.identity-and-aliases",
            "Modules, imports, aliases, namespaces, and reexports resolve to one source-observable identity.",
            1042,
            "Canonical source identity is a prerequisite for a replacement profile that crosses module boundaries.",
        ),
        planned_feature_with_bounded_cases(
            "nominal.models-unions-enums",
            "Models, unions, value enums, newtypes, computed properties, and static storage preserve construction and dispatch semantics.",
            988,
            "#1281 retains and executes the bounded checked int/bool/str/float `isinstance` target profile in replacement-body-v0-030. That case does not establish general runtime type values or the wider models/unions/enums/newtypes contract. Closed #1154 delivered the current direct nominal/value substrate; open #988 owns broadening the replacement execution profile.",
        ),
        planned_feature(
            "package.public-boundaries",
            "Libraries, checked API metadata, providers, workspaces, and consumer imports preserve public identity and defaults.",
            989,
            "Package and ABI boundaries deliberately remain outside the direct source-only profile until #656/#989 evidence exists.",
        ),
        planned_feature(
            "runtime.std-data-services",
            "Data-oriented stdlib services preserve their documented input, output, and error contracts.",
            988,
            "Closed #1156 delivered one checked provider-service dispatch and closed #1154 delivered its value-state prerequisite; open #988 owns broadening direct data-service execution and comparison.",
        ),
        planned_feature(
            "runtime.std-hosted-services",
            "Hosted filesystem, environment, I/O, web, temporary-resource, and process-adjacent services retain authority and lifecycle semantics.",
            988,
            "Closed #1156 delivered one checked provider-service dispatch. Open #988 owns broader direct execution and comparison, with authority and receipt facts still supplied by #662.",
        ),
        planned_feature(
            "runtime.std-observability",
            "Logging, telemetry, registries, and metadata services preserve structured values and provider behavior.",
            988,
            "Closed #1156 delivered one checked provider-service dispatch; open #988 owns broader direct observability execution and comparison, while provider authority and receipts remain explicit prerequisites.",
        ),
        planned_feature(
            "testing-and-tooling",
            "Test discovery, assertions, formatter, build reports, inspection, lifecycle, installer, and Oven observability preserve documented contracts.",
            1034,
            "These are control-plane contracts with source and receipt evidence, not direct Body-IR execution rows.",
        ),
        planned_feature(
            "types.traits-generics-reflection",
            "Traits, generics, type tokens, protocol hooks, derives, and resolved method signatures preserve checked dispatch decisions.",
            1033,
            "Type-directed runtime calls and reflection need canonical source facts and value representation beyond the current profile.",
        ),
        planned_feature(
            "interop.rust-and-c",
            "Rust and C boundaries preserve checked signatures, coercions, explicit unsafe acknowledgements, and source-map diagnostics.",
            989,
            "Public ABI and interop parity is an explicit replacement-boundary slice, not a direct scalar-executor extension.",
        ),
    ]
}

/// Return the temporary crosswalk records that await migration into an owning compiler boundary.
fn migration_bootstrap_compatibility_contribution() -> ReplacementCompatibilityContribution {
    migration_bootstrap_contribution(
        "replacement-compatibility.migration-bootstrap",
        "src/replacement_compatibility.rs",
        "fn migration_bootstrap_compatibility_contribution",
        "Retire this contributor when every remaining feature and requirement has moved to the module that implements its coherent mechanism; then retain the v0.5 source only as an explicitly historical regression fixture if a later migration needs it.",
        migration_bootstrap_compatibility_features(),
        migration_bootstrap_implementation_requirements(),
        public_capability_links(),
        migration_bootstrap_feature_requirement_links(),
    )
}

/// Audited extension points for a feature family's typechecker, Body-IR, and direct-runtime coverage.
struct FeatureAnchorProfile {
    typechecker_path: &'static str,
    typechecker_selector: &'static str,
    body_ir_selector: &'static str,
    replacement_executor_selector: &'static str,
}

/// Return the audited source/typechecker, Body-IR, and direct-runtime extension points for a feature family.
///
/// These are intentionally feature-family-specific rather than a generic call anchor. The planned Body-IR and
/// executor selectors identify existing seams where an owner must materialize coverage; they do not assert support.
fn feature_anchor_profile(feature_id: &str) -> FeatureAnchorProfile {
    match feature_id {
        "call.named-and-variadic" | "call.partial-binding" | "call.stored-callables" => FeatureAnchorProfile {
            typechecker_path: "src/frontend/typechecker/check_expr/calls.rs",
            typechecker_selector: "fn check_call",
            body_ir_selector: "fn lower_call",
            replacement_executor_selector: "fn execute_call",
        },
        "decorators.dsl-surfaces" => FeatureAnchorProfile {
            typechecker_path: "src/frontend/typechecker/collect/decorators.rs",
            typechecker_selector: "fn validate_decorators_allowing_user_defined",
            body_ir_selector: "fn lower_function_body",
            replacement_executor_selector: "fn execute_call",
        },
        "diagnostics.stable" => FeatureAnchorProfile {
            typechecker_path: "src/frontend/typechecker/check_stmt.rs",
            typechecker_selector: "fn check_statement",
            body_ir_selector: "fn lower_function_body",
            replacement_executor_selector: "fn execute_free_function",
        },
        "error.result-and-try" => FeatureAnchorProfile {
            typechecker_path: "src/frontend/typechecker/check_expr/control_flow.rs",
            typechecker_selector: "fn check_try",
            body_ir_selector: "fn lower_try",
            replacement_executor_selector: "fn execute_call",
        },
        "generator.expressions" | "generator.functions" => FeatureAnchorProfile {
            typechecker_path: "src/frontend/typechecker/check_expr/calls.rs",
            typechecker_selector: "fn check_call",
            body_ir_selector: "fn lower_generator_expr",
            replacement_executor_selector: "ReplacementGenerator",
        },
        "iteration.protocol-and-adapters" | "iteration.user-and-fallible" => FeatureAnchorProfile {
            typechecker_path: "src/frontend/typechecker/check_expr/ops.rs",
            typechecker_selector: "fn resolve_iteration_protocol",
            body_ir_selector: "fn lower_general_iteration",
            replacement_executor_selector: "fn execute_loop",
        },
        "language.aggregates-and-projections" => FeatureAnchorProfile {
            typechecker_path: "src/frontend/typechecker/check_expr/collections.rs",
            typechecker_selector: "fn check_list",
            body_ir_selector: "fn lower_aggregate",
            replacement_executor_selector: "fn evaluate_aggregate",
        },
        "language.control-flow-complete" => FeatureAnchorProfile {
            typechecker_path: "src/frontend/typechecker/check_expr/control_flow.rs",
            typechecker_selector: "fn check_if_expr",
            body_ir_selector: "fn lower_if_expr",
            replacement_executor_selector: "fn execute_loop",
        },
        "language.control-flow" => FeatureAnchorProfile {
            typechecker_path: "src/frontend/typechecker/check_expr/control_flow.rs",
            typechecker_selector: "fn check_if_expr",
            body_ir_selector: "fn lower_if",
            replacement_executor_selector: "fn execute_loop",
        },
        "language.match-and-patterns" => FeatureAnchorProfile {
            typechecker_path: "src/frontend/typechecker/check_expr/match_.rs",
            typechecker_selector: "fn check_match",
            body_ir_selector: "fn lower_match",
            replacement_executor_selector: "fn execute_call",
        },
        "language.numeric-and-scalar" | "language.numeric-complete" | "language.strings-and-format" => {
            FeatureAnchorProfile {
                typechecker_path: "src/frontend/typechecker/check_expr/ops.rs",
                typechecker_selector: "fn check_binary",
                body_ir_selector: "fn lower_binary",
                replacement_executor_selector: "fn evaluate_binary",
            }
        }
        "module.identity-and-aliases" => FeatureAnchorProfile {
            typechecker_path: "src/frontend/typechecker/collect/stdlib_imports.rs",
            typechecker_selector: "fn collect_import",
            body_ir_selector: "fn lower_call",
            replacement_executor_selector: "fn execute_call",
        },
        "nominal.models-unions-enums" => FeatureAnchorProfile {
            typechecker_path: "src/frontend/typechecker/check_decl.rs",
            typechecker_selector: "fn check_model",
            body_ir_selector: "fn lower_constructor",
            replacement_executor_selector: "fn evaluate_aggregate",
        },
        "package.public-boundaries" => FeatureAnchorProfile {
            typechecker_path: "src/frontend/typechecker/collect/stdlib_imports.rs",
            typechecker_selector: "fn collect_pub_imports",
            body_ir_selector: "fn lower_call",
            replacement_executor_selector: "fn execute_call",
        },
        "runtime.std-data-services" | "runtime.std-hosted-services" | "runtime.std-observability" => {
            FeatureAnchorProfile {
                typechecker_path: "src/frontend/typechecker/stdlib_loader.rs",
                typechecker_selector: "fn lookup_function_symbol",
                body_ir_selector: "fn lower_call",
                replacement_executor_selector: "fn execute_call",
            }
        }
        "testing-and-tooling" => FeatureAnchorProfile {
            typechecker_path: "src/frontend/typechecker/check_decl.rs",
            typechecker_selector: "fn check_test_module",
            body_ir_selector: "fn lower_function_body",
            replacement_executor_selector: "fn execute_free_function",
        },
        "types.traits-generics-reflection" => FeatureAnchorProfile {
            typechecker_path: "src/frontend/typechecker/trait_bound_relations.rs",
            typechecker_selector: "fn type_satisfies_explicit_bound",
            body_ir_selector: "fn lower_call",
            replacement_executor_selector: "fn execute_call",
        },
        "interop.rust-and-c" => FeatureAnchorProfile {
            typechecker_path: "src/frontend/typechecker/check_expr/calls/rust_boundary.rs",
            typechecker_selector: "fn validate_rust_boundary_value",
            body_ir_selector: "fn lower_call",
            replacement_executor_selector: "fn execute_call",
        },
        _ => FeatureAnchorProfile {
            typechecker_path: "src/frontend/typechecker/check_expr/calls.rs",
            typechecker_selector: "fn check_call",
            body_ir_selector: "fn lower_call",
            replacement_executor_selector: "fn execute_call",
        },
    }
}

/// Build one planned in-envelope feature with an explicit positive/negative probe family and blocker.
fn planned_feature(
    id: &'static str,
    contract: &'static str,
    owner_issue: u32,
    blocker: &'static str,
) -> CompatibilityFeature {
    let profile = feature_anchor_profile(id);
    planned_feature_at_boundary(
        id,
        contract,
        owner_issue,
        blocker,
        profile.typechecker_path,
        profile.typechecker_selector,
        profile.body_ir_selector,
        profile.replacement_executor_selector,
    )
}

/// Build a broad planned feature while retaining independently executable bounded corpus cases.
///
/// The feature-wide Body-IR, direct-execution, and comparison states remain non-green. Registered cases and their
/// paired receipts describe only the named bounded probes, so landed evidence is not hidden behind the remaining
/// feature work and does not promote that wider contract to preserved.
fn planned_feature_with_bounded_cases(
    id: &'static str,
    contract: &'static str,
    owner_issue: u32,
    blocker: &'static str,
) -> CompatibilityFeature {
    let mut feature = planned_feature(id, contract, owner_issue, blocker);
    let case_ids = registered_parity_case_ids(id);
    let anchor_selector = case_ids.first().copied().unwrap_or("fn seed_corpus");
    feature.evidence.surfaces.parity_corpus = ParityCorpusReference::Registered {
        case_ids: case_ids.into_iter().map(str::to_string).collect(),
        anchor: observed_anchor(
            EvidenceSurface::ParityCorpus,
            "tests/parity_corpus_tests.rs",
            anchor_selector,
            "The stable #987 corpus registers only the bounded direct profiles already executable inside this broader planned feature.",
        ),
    };
    feature.evidence.surfaces.scoped_comparisons = scoped_comparisons(id);
    feature
}

/// Build one planned feature at the module that owns its current compiler boundary.
///
/// The bootstrap map uses [`planned_feature`] only until an implementation has a coherent local registration. New
/// implementation work must call this constructor from that boundary with its own audited selectors rather than
/// extending the migration profile switch above.
///
/// Each parameter is a distinct audited anchor for the feature being registered, so grouping them into a struct would
/// only move the same list behind another type.
#[allow(clippy::too_many_arguments)]
pub(crate) fn planned_feature_at_boundary(
    id: &'static str,
    contract: &'static str,
    owner_issue: u32,
    blocker: &'static str,
    typechecker_path: &'static str,
    typechecker_selector: &'static str,
    body_ir_selector: &'static str,
    replacement_executor_selector: &'static str,
) -> CompatibilityFeature {
    let profile = FeatureAnchorProfile {
        typechecker_path,
        typechecker_selector,
        body_ir_selector,
        replacement_executor_selector,
    };
    let source_ast = observed_anchor(
        EvidenceSurface::SourceAst,
        FROZEN_V0_5_CAPABILITIES_PATH,
        "FeatureId",
        "The frozen public registry is the initial source-contract crosswalk; construction replaces this with a linked descriptor ID.",
    );
    let typechecker = observed_anchor(
        EvidenceSurface::Typechecker,
        profile.typechecker_path,
        profile.typechecker_selector,
        "The audited typechecker boundary is the current admission or diagnostic seam; feature-specific positive and negative probes remain owned materialization work.",
    );
    CompatibilityFeature {
        id: id.to_string(),
        contract: contract.to_string(),
        probes: vec![SourceProbe {
            id: format!("probe:{id}:binding-and-refusal"),
            positive: ProbeExpectation {
                outcome: ProbeOutcome::AcceptedBehavior,
                contract: contract.to_string(),
                anchor: source_ast.clone(),
            },
            negative: ProbeExpectation {
                outcome: ProbeOutcome::IntentionalRefusal,
                contract: "Reject unsupported variants with an intentional source-owned diagnostic and no silent legacy fallback.".to_string(),
                anchor: typechecker.clone(),
            },
        }],
        evidence: CompatibilityEvidence {
            source_contract: SourceContractState::Checked,
            legacy_run: LegacyRunState::Unknown,
            body_ir: BodyIrRepresentationState::Partial,
            direct_replacement: DirectReplacementOutcome::BlockedByRequirements,
            independent_comparison: IndependentComparisonState::NonGreenShadowUnavailable,
            surfaces: FeatureSurfaceCoverage {
                source_ast,
                typechecker,
                body_ir: planned_anchor(
                    EvidenceSurface::BodyIr,
                    "src/frontend/body_ir.rs",
                    profile.body_ir_selector,
                    owner_issue,
                    "The Body-IR lowering boundary is the materialization point for the planned source contract.",
                ),
                replacement_executor: planned_anchor(
                    EvidenceSurface::ReplacementExecutor,
                    "src/backend/replacement/mod.rs",
                    profile.replacement_executor_selector,
                    owner_issue,
                    "The direct replacement call boundary remains planned or intentionally refused until this feature's requirements land.",
                ),
                parity_corpus: ParityCorpusReference::Planned {
                    case_id: format!("parity-987-plan-{id}"),
                    owner_issue: 987,
                    anchor: planned_anchor(
                        EvidenceSurface::ParityCorpus,
                        "tests/parity_corpus_tests.rs",
                        "fn seed_corpus",
                        987,
                        "#987 owns materializing the reserved stable corpus case with direct/refusal evidence.",
                    ),
                },
                independent_comparison: ComparisonEvidence::Unavailable {
                    comparison_infrastructure: completed_comparison_infrastructure(),
                    outstanding_evidence: OutstandingComparisonEvidence::Scheduled {
                        owner_issue,
                        note: "The feature/runtime owner must add receipt-bound comparison evidence after its direct profile is materialized."
                            .to_string(),
                    },
                },
                scoped_comparisons: Vec::new(),
            },
        },
        disposition: CompatibilityDisposition::Planned,
        owner_issue: Some(owner_issue),
        migration_or_blocker: Some(blocker.to_string()),
    }
}

/// Build one partially materialized feature at the implementation boundary that owns its admitted subset.
///
/// Both compiler representation and direct execution have observed seams, but the feature-wide contract stays
/// non-green and blocked on its named follow-up. Registered corpus comparisons prove only their bounded rows.
#[allow(clippy::too_many_arguments)]
pub(crate) fn partially_materialized_feature_at_boundary(
    id: &'static str,
    contract: &'static str,
    owner_issue: u32,
    blocker: &'static str,
    typechecker_path: &'static str,
    typechecker_selector: &'static str,
    body_ir_path: &'static str,
    body_ir_selector: &'static str,
    replacement_executor_selector: &'static str,
) -> CompatibilityFeature {
    let mut feature = planned_feature_at_boundary(
        id,
        contract,
        owner_issue,
        blocker,
        typechecker_path,
        typechecker_selector,
        body_ir_selector,
        replacement_executor_selector,
    );
    let case_ids = registered_parity_case_ids(id);
    let direct_selector = case_ids.first().copied().unwrap_or("fn seed_corpus");
    feature.evidence.surfaces.body_ir = observed_anchor(
        EvidenceSurface::BodyIr,
        body_ir_path,
        body_ir_selector,
        "Body IR represents the admitted subset at this compiler-owned lowering seam.",
    );
    feature.evidence.surfaces.replacement_executor = observed_anchor(
        EvidenceSurface::ReplacementExecutor,
        "src/backend/replacement/mod.rs",
        replacement_executor_selector,
        "The replacement executor admits the bounded subset and refuses the remaining feature surface before effects.",
    );
    feature.evidence.surfaces.parity_corpus = ParityCorpusReference::Registered {
        case_ids: case_ids.into_iter().map(str::to_string).collect(),
        anchor: observed_anchor(
            EvidenceSurface::ParityCorpus,
            "tests/parity_corpus_tests.rs",
            direct_selector,
            "The stable #987 corpus registers only the bounded source-observable subset already executable here.",
        ),
    };
    feature.evidence.surfaces.scoped_comparisons = scoped_comparisons(id);
    feature
}

/// Build one bounded direct-execution feature at the direct implementation boundary that owns it.
///
/// Direct execution remains separate from comparison. This factory never widens a feature to comparison-green; that
/// still requires the receipt-bound evidence validated by the collector.
pub(crate) fn preserved_feature_at_boundary(
    id: &'static str,
    contract: &'static str,
    typechecker_path: &'static str,
    typechecker_selector: &'static str,
    body_ir_selector: &'static str,
    replacement_executor_selector: &'static str,
) -> CompatibilityFeature {
    let profile = FeatureAnchorProfile {
        typechecker_path,
        typechecker_selector,
        body_ir_selector,
        replacement_executor_selector,
    };
    let source_ast = observed_anchor(
        EvidenceSurface::SourceAst,
        FROZEN_V0_5_CAPABILITIES_PATH,
        "FeatureId",
        "The frozen public registry is the initial source-contract crosswalk; construction replaces this with a linked descriptor ID.",
    );
    let typechecker = observed_anchor(
        EvidenceSurface::Typechecker,
        profile.typechecker_path,
        profile.typechecker_selector,
        "The audited typechecker boundary supplies the current source admission anchor for this bounded direct profile.",
    );
    let case_ids = registered_parity_case_ids(id);
    let direct_selector = case_ids.first().copied().unwrap_or("replacement-body-v0-001");
    CompatibilityFeature {
        id: id.to_string(),
        contract: contract.to_string(),
        probes: vec![SourceProbe {
            id: format!("probe:{id}:bounded-direct-profile"),
            positive: ProbeExpectation {
                outcome: ProbeOutcome::AcceptedBehavior,
                contract: contract.to_string(),
                anchor: source_ast.clone(),
            },
            negative: ProbeExpectation {
                outcome: ProbeOutcome::IntentionalRefusal,
                contract: "Inputs outside the bounded direct profile refuse visibly with their source span."
                    .to_string(),
                anchor: typechecker.clone(),
            },
        }],
        evidence: CompatibilityEvidence {
            source_contract: SourceContractState::Checked,
            legacy_run: LegacyRunState::Unknown,
            body_ir: BodyIrRepresentationState::Represented,
            direct_replacement: DirectReplacementOutcome::Executable,
            independent_comparison: IndependentComparisonState::NonGreenShadowUnavailable,
            surfaces: FeatureSurfaceCoverage {
                source_ast,
                typechecker,
                body_ir: observed_anchor(
                    EvidenceSurface::BodyIr,
                    "src/frontend/body_ir.rs",
                    profile.body_ir_selector,
                    "Body IR lowers the currently bounded direct profile through compiler-owned operation records.",
                ),
                replacement_executor: observed_anchor(
                    EvidenceSurface::ReplacementExecutor,
                    "src/backend/replacement/mod.rs",
                    profile.replacement_executor_selector,
                    "The replacement evaluator seam executes the currently bounded direct profile; the #987 corpus is recorded separately.",
                ),
                parity_corpus: ParityCorpusReference::Registered {
                    case_ids: case_ids.into_iter().map(str::to_string).collect(),
                    anchor: observed_anchor(
                        EvidenceSurface::ParityCorpus,
                        "tests/parity_corpus_tests.rs",
                        direct_selector,
                        "The stable #987 seed corpus registers this bounded direct-execution evidence.",
                    ),
                },
                independent_comparison: ComparisonEvidence::Unavailable {
                    comparison_infrastructure: completed_comparison_infrastructure(),
                    outstanding_evidence: OutstandingComparisonEvidence::UnscheduledDebt {
                        note: "The bounded direct profile has no scheduled owner for its remaining aggregate and corpus-case comparison evidence."
                            .to_string(),
                    },
                },
                scoped_comparisons: scoped_comparisons(id),
            },
        },
        disposition: CompatibilityDisposition::Preserved,
        owner_issue: None,
        migration_or_blocker: None,
    }
}

/// Return #1146's completed reusable comparison infrastructure without assigning it outstanding evidence work.
fn completed_comparison_infrastructure() -> CompletedComparisonInfrastructure {
    CompletedComparisonInfrastructure {
        issue: COMPLETED_COMPARISON_INFRASTRUCTURE_ISSUE,
        anchor: observed_anchor(
            EvidenceSurface::IndependentComparison,
            "tests/support/parity_corpus.rs",
            "NonGreenShadowUnavailable",
            "#1146 completed the reusable paired-comparison route; outstanding case and aggregate evidence has separate ownership.",
        ),
    }
}

/// Return case-scoped comparisons, each proving one registered corpus case without widening the feature.
fn scoped_comparisons(feature_id: &str) -> Vec<CorpusCaseComparisonEvidence> {
    match feature_id {
        "iteration.protocol-and-adapters" => vec![paired_scoped_comparison(
            "replacement-body-v0-023",
            "fn the_enumerate_zip_row_carries_two_route_receipts_and_exact_output",
            "canonical list-based enumerate and Zip",
        )],
        "language.aggregates-and-projections" => vec![
            paired_scoped_comparison(
                "replacement-body-v0-020",
                "fn the_hashed_membership_row_carries_two_route_receipts_and_exact_output",
                "scalar-key set and dict membership",
            ),
            paired_scoped_comparison(
                "replacement-body-v0-026",
                "fn the_collection_len_row_carries_two_route_receipts_and_exact_output",
                "distinct set-entry and dict-key counts",
            ),
            paired_scoped_comparison(
                "replacement-body-v0-028",
                "fn the_sorted_int_list_row_carries_two_route_receipts_and_exact_output",
                "nonempty integer-list sorting",
            ),
        ],
        "nominal.models-unions-enums" => vec![paired_scoped_comparison(
            "replacement-body-v0-030",
            "fn the_isinstance_targets_row_carries_two_route_receipts_and_exact_output",
            "checked int/bool/str/float isinstance targets over source-local union values",
        )],
        "language.strings-and-format" => vec![
            paired_scoped_comparison(
                "replacement-body-v0-021",
                "fn the_string_helper_row_carries_two_route_receipts_and_exact_output",
                "selected canonical string helpers",
            ),
            paired_scoped_comparison(
                "replacement-body-v0-024",
                "fn the_string_len_row_carries_two_route_receipts_and_exact_output",
                "Unicode-scalar string length",
            ),
        ],
        "language.numeric-and-scalar" => vec![
            CorpusCaseComparisonEvidence {
                case_id: "replacement-body-v0-001".to_string(),
                state: IndependentComparisonState::ComparedMatch,
                evidence: ComparisonEvidence::Paired {
                    legacy_receipt: observed_anchor(
                        EvidenceSurface::IndependentComparison,
                        "tests/parity_corpus_tests.rs",
                        "legacy_receipt_identity",
                        "#1146 verifies the legacy Oven route's receipt identity for replacement-body-v0-001.",
                    ),
                    replacement_receipt: observed_anchor(
                        EvidenceSurface::IndependentComparison,
                        "tests/parity_corpus_tests.rs",
                        "replacement_receipt_identity",
                        "#1146 verifies the direct replacement route's receipt identity for replacement-body-v0-001.",
                    ),
                    comparison_record: observed_anchor(
                        EvidenceSurface::IndependentComparison,
                        "tests/parity_corpus_tests.rs",
                        "fn the_compared_row_carries_two_route_receipts_and_its_oven_authority",
                        "#1146 records the matched two-route source observable for replacement-body-v0-001.",
                    ),
                },
            },
            CorpusCaseComparisonEvidence {
                case_id: "replacement-body-v0-022".to_string(),
                state: IndependentComparisonState::ComparedMatch,
                evidence: ComparisonEvidence::Paired {
                    legacy_receipt: observed_anchor(
                        EvidenceSurface::IndependentComparison,
                        "tests/parity_corpus_tests.rs",
                        "fn the_scalar_conversions_row_carries_two_route_receipts_and_exact_output",
                        "#1249 verifies the legacy Oven route receipt for replacement-body-v0-022.",
                    ),
                    replacement_receipt: observed_anchor(
                        EvidenceSurface::IndependentComparison,
                        "tests/parity_corpus_tests.rs",
                        "fn the_scalar_conversions_row_carries_two_route_receipts_and_exact_output",
                        "#1249 verifies the direct replacement route receipt for replacement-body-v0-022.",
                    ),
                    comparison_record: observed_anchor(
                        EvidenceSurface::IndependentComparison,
                        "tests/parity_corpus_tests.rs",
                        "fn the_scalar_conversions_row_carries_two_route_receipts_and_exact_output",
                        "#1249 records the matched typed result and exact streams for replacement-body-v0-022.",
                    ),
                },
            },
            CorpusCaseComparisonEvidence {
                case_id: "replacement-body-v0-025".to_string(),
                state: IndependentComparisonState::ComparedMatch,
                evidence: ComparisonEvidence::Paired {
                    legacy_receipt: observed_anchor(
                        EvidenceSurface::IndependentComparison,
                        "tests/parity_corpus_tests.rs",
                        "fn the_scalar_json_row_carries_two_route_receipts_and_exact_output",
                        "#1249 verifies the legacy Oven route receipt for replacement-body-v0-025.",
                    ),
                    replacement_receipt: observed_anchor(
                        EvidenceSurface::IndependentComparison,
                        "tests/parity_corpus_tests.rs",
                        "fn the_scalar_json_row_carries_two_route_receipts_and_exact_output",
                        "#1249 verifies the direct replacement route receipt for replacement-body-v0-025.",
                    ),
                    comparison_record: observed_anchor(
                        EvidenceSurface::IndependentComparison,
                        "tests/parity_corpus_tests.rs",
                        "fn the_scalar_json_row_carries_two_route_receipts_and_exact_output",
                        "#1249 records exact scalar JSON bytes and the matched two-route source observable for replacement-body-v0-025.",
                    ),
                },
            },
            CorpusCaseComparisonEvidence {
                case_id: "replacement-body-v0-027".to_string(),
                state: IndependentComparisonState::ComparedMatch,
                evidence: ComparisonEvidence::Paired {
                    legacy_receipt: observed_anchor(
                        EvidenceSurface::IndependentComparison,
                        "tests/parity_corpus_tests.rs",
                        "fn the_bool_truthiness_row_carries_two_route_receipts_and_exact_output",
                        "#1249 verifies the legacy Oven route receipt for replacement-body-v0-027.",
                    ),
                    replacement_receipt: observed_anchor(
                        EvidenceSurface::IndependentComparison,
                        "tests/parity_corpus_tests.rs",
                        "fn the_bool_truthiness_row_carries_two_route_receipts_and_exact_output",
                        "#1249 verifies the direct replacement route receipt for replacement-body-v0-027.",
                    ),
                    comparison_record: observed_anchor(
                        EvidenceSurface::IndependentComparison,
                        "tests/parity_corpus_tests.rs",
                        "fn the_bool_truthiness_row_carries_two_route_receipts_and_exact_output",
                        "#1249 records bounded canonical truthiness and exact streams for replacement-body-v0-027.",
                    ),
                },
            },
        ],
        "language.numeric-complete" => vec![paired_scoped_comparison(
            "replacement-body-v0-029",
            "fn the_typed_numeric_row_carries_exact_type_and_two_route_receipts",
            "exact-width and decimal carrier transport",
        )],
        _ => Vec::new(),
    }
}

/// Build one paired comparison record whose evidence remains confined to a single stable corpus case.
fn paired_scoped_comparison(
    case_id: &'static str,
    test_selector: &'static str,
    bounded_contract: &'static str,
) -> CorpusCaseComparisonEvidence {
    CorpusCaseComparisonEvidence {
        case_id: case_id.to_string(),
        state: IndependentComparisonState::ComparedMatch,
        evidence: ComparisonEvidence::Paired {
            legacy_receipt: observed_anchor(
                EvidenceSurface::IndependentComparison,
                "tests/parity_corpus_tests.rs",
                test_selector,
                &format!(
                    "The paired corpus test verifies the legacy Oven receipt for {bounded_contract} in {case_id}."
                ),
            ),
            replacement_receipt: observed_anchor(
                EvidenceSurface::IndependentComparison,
                "tests/parity_corpus_tests.rs",
                test_selector,
                &format!(
                    "The paired corpus test verifies the direct replacement receipt for {bounded_contract} in {case_id}."
                ),
            ),
            comparison_record: observed_anchor(
                EvidenceSurface::IndependentComparison,
                "tests/parity_corpus_tests.rs",
                test_selector,
                &format!(
                    "The paired corpus test records matching exact streams and the typed result for {bounded_contract} in {case_id}."
                ),
            ),
        },
    }
}

/// Return the reviewed stable #987 rows that cover each bounded direct profile.
fn registered_parity_case_ids(feature_id: &str) -> Vec<&'static str> {
    match feature_id {
        "language.control-flow" => vec![
            "replacement-body-v0-004",
            "replacement-body-v0-005",
            "replacement-body-v0-006",
        ],
        "language.numeric-and-scalar" => vec![
            "replacement-body-v0-001",
            "replacement-body-v0-002",
            "replacement-body-v0-003",
            "replacement-body-v0-005",
            "replacement-body-v0-022",
            "replacement-body-v0-025",
            "replacement-body-v0-027",
        ],
        "language.numeric-complete" => vec!["replacement-body-v0-029"],
        "iteration.protocol-and-adapters" => vec!["replacement-body-v0-023"],
        "language.aggregates-and-projections" => vec![
            "replacement-body-v0-020",
            "replacement-body-v0-026",
            "replacement-body-v0-028",
        ],
        "nominal.models-unions-enums" => vec!["replacement-body-v0-030"],
        "language.strings-and-format" => vec!["replacement-body-v0-021", "replacement-body-v0-024"],
        "async.tasks" => vec!["replacement-body-v0-018", "replacement-body-v0-019"],
        _ => Vec::new(),
    }
}

/// Construct temporary private requirements that have not reached their owning local implementation boundary yet.
fn migration_bootstrap_implementation_requirements() -> Vec<ImplementationRequirement> {
    vec![
        requirement(
            "call.frames",
            "Every local or nested callable has isolated locals, return flow, and source-owned spans.",
            "replacement runtime call dispatcher",
            "stored-callable Body-IR tests and #1152 execution probes",
            "Frames enable several callable features without being user-visible themselves.",
        ),
        requirement(
            "diagnostics.source-authority",
            "Refusals and failures retain original source spans and intentional diagnostic categories.",
            "frontend diagnostics and backend selection receipts",
            "diagnostic fixtures and #987 classifier",
            "Diagnostic routing is a compiler mechanism, not a separate capability.",
        ),
        requirement(
            "error.result-routing",
            "Success/error values, propagation, and handler ordering survive direct execution.",
            "Body IR try lowering and replacement runtime",
            "try/result Body-IR and runtime probes",
            "Error routing serves many source spellings.",
        ),
        requirement(
            "iteration.protocol-dispatch",
            "Iterator acquisition, next polling, exhaustion, and fallible routing use compiler-resolved protocol facts.",
            "typechecker protocol facts, Body IR, replacement runtime",
            "iteration protocol Body-IR snapshots",
            "Protocol dispatch is private shared machinery.",
        ),
        requirement(
            "modules.canonical-identity",
            "Imports, aliases, and calls use canonical source identities rather than generated-Rust spellings.",
            "module resolver and semantic facts",
            "canonical identity and package-boundary tests",
            "Canonical identity is a compiler fact.",
        ),
        requirement(
            "nominal.value-model",
            "Nominal instances, fields, discriminators, defaults, and statics have direct runtime representation.",
            "Body IR value lowering and replacement runtime",
            "model/union/value-enum Body-IR probes",
            "The value model is internal runtime machinery.",
        ),
        requirement(
            "packages.public-contract",
            "Public API, ABI, source maps, library manifests, and provider identity remain versioned semantic contracts.",
            "package/ABI boundary",
            "#989 boundary corpus",
            "Package machinery has no one-to-one public capability.",
        ),
        requirement(
            "patterns.dispatch",
            "Pattern alternatives, destructuring, guards, and exhaustiveness route through explicit value tests.",
            "typechecker match facts and Body IR",
            "match diagnostics and Body-IR tests",
            "Pattern dispatch is shared execution machinery.",
        ),
        requirement(
            "providers.runtime-services",
            "Stdlib providers expose checked activation, service values, authority, and runtime errors.",
            "stdlib/provider plan and replacement runtime",
            "stdlib/provider and authority tests",
            "Provider planning is private runtime infrastructure.",
        ),
        requirement(
            "receipts.comparison",
            "Selection, execution, and comparison use matching identities and receipts; unavailable comparison remains non-green.",
            "backend selection, #1146, and #987",
            "backend selection and parity corpus tests",
            "Receipts prove execution provenance rather than expose a language feature.",
        ),
        requirement(
            "runtime.aggregate-store",
            "Aggregates, projections, mutation, hashing, equality, and ordering use explicit value storage.",
            "Body IR aggregates/places and replacement runtime",
            "aggregate and assignment Body-IR tests",
            "Aggregate storage serves multiple public collection forms.",
        ),
        requirement(
            "suspension.continuations",
            "Generator and lazy adapter state preserve locals, instruction position, and observed effects across resume.",
            "Body IR generator model and replacement runtime",
            "generator laziness and resume probes",
            "Continuations are private runtime state.",
        ),
        requirement(
            "surface.decorator-dispatch",
            "Activated surface packs and decorators preserve their typecheck/lowering dispatch ownership.",
            "semantic registry and surface semantics packs",
            "surface semantics and vocab tests",
            "Dispatch packs are compiler routing mechanics.",
        ),
        requirement(
            "testing.tooling-control-plane",
            "Formatter, testing, build, inspection, lifecycle, and Oven facts remain source/receipt owned.",
            "CLI, Oven, and test-runner boundaries",
            "CLI and Oven integration tests",
            "Tooling coordination is not a direct expression runtime.",
        ),
        requirement(
            "types.resolved-dispatch",
            "Resolved types, trait bounds, generic arguments, methods, and type tokens reach execution without AST rediscovery.",
            "typechecker facts and Body IR call model",
            "typechecker resolution and type-token tests",
            "Resolved dispatch is an internal semantic fact.",
        ),
        requirement(
            "unsafe.interop-boundary",
            "Rust/C calls retain checked signatures, coercions, explicit unsafe authority, and source maps.",
            "interop metadata and package boundary",
            "Rust/C interop and package consumer tests",
            "Interop planning is a private compiler boundary.",
        ),
    ]
}

/// Build one private requirement at its owning compiler boundary.
pub(crate) fn implementation_requirement(
    id: &'static str,
    invariant: &'static str,
    owner_boundary: &'static str,
    verification_anchor: &'static str,
    internal_only_rationale: &'static str,
) -> ImplementationRequirement {
    ImplementationRequirement {
        id,
        invariant,
        owner_boundary,
        verification_anchor,
        internal_only_rationale,
    }
}

/// Build one bootstrap-private requirement through the same local-registration shape.
fn requirement(
    id: &'static str,
    invariant: &'static str,
    owner_boundary: &'static str,
    verification_anchor: &'static str,
    internal_only_rationale: &'static str,
) -> ImplementationRequirement {
    implementation_requirement(
        id,
        invariant,
        owner_boundary,
        verification_anchor,
        internal_only_rationale,
    )
}

/// Construct the complete v0.5 public-capability to compatibility-feature relation.
fn public_capability_links() -> Vec<PublicFeatureLink> {
    vec![
        link("AbstractTraits", "types.traits-generics-reflection"),
        link("AsyncAwait", "async.tasks"),
        link("AsyncRace", "async.tasks"),
        link("BuildReportsAndRustInspection", "testing-and-tooling"),
        link("BuildTestOvenObservability", "testing-and-tooling"),
        link("CallSiteGenerics", "call.named-and-variadic"),
        link("CallablePresets", "call.partial-binding"),
        link("CheckedApiMetadata", "package.public-boundaries"),
        link("CheckedCBindingFoundation", "interop.rust-and-c"),
        link("CodegraphInspection", "testing-and-tooling"),
        link(
            "CompiledProvidersSdkComponentsPackageFeatures",
            "package.public-boundaries",
        ),
        link("ComputedProperties", "nominal.models-unions-enums"),
        link("EnumMethodsTraits", "nominal.models-unions-enums"),
        link("FallibleIteration", "iteration.user-and-fallible"),
        link("FirstClassFunctions", "call.named-and-variadic"),
        link("FirstClassFunctions", "call.stored-callables"),
        link("FormatterContract", "testing-and-tooling"),
        link("Generators", "generator.expressions"),
        link("Generators", "generator.functions"),
        link("IfWhileLet", "language.control-flow"),
        link("IfWhileLet", "language.control-flow-complete"),
        link("IncanLibraries", "package.public-boundaries"),
        link("IteratorAdapters", "iteration.protocol-and-adapters"),
        link("LoopExpressions", "language.control-flow"),
        link("LoopExpressions", "language.control-flow-complete"),
        link("ModelFieldMetadata", "nominal.models-unions-enums"),
        link("NamespacedStdlib", "module.identity-and-aliases"),
        link("NumericTypeSystem", "language.numeric-and-scalar"),
        link("NumericTypeSystem", "language.numeric-complete"),
        link("NumericTypeSystem", "language.strings-and-format"),
        link("OvenInteropRequirements", "package.public-boundaries"),
        link("PatternAlternation", "language.match-and-patterns"),
        link("ProjectLifecycle", "testing-and-tooling"),
        link("ProtocolHooks", "types.traits-generics-reflection"),
        link("ResultCombinators", "error.result-and-try"),
        link("RustAllow", "interop.rust-and-c"),
        link("RustInteropBoundary", "interop.rust-and-c"),
        link("RustTraitAdoption", "types.traits-generics-reflection"),
        link("ScopedDslSurfaces", "decorators.dsl-surfaces"),
        link("SourceDefinedDerivesTraits", "types.traits-generics-reflection"),
        link("StableDiagnostics", "diagnostics.stable"),
        link("StaticStorage", "nominal.models-unions-enums"),
        link("StdChecksum", "runtime.std-data-services"),
        link("StdCollections", "language.aggregates-and-projections"),
        link("StdCompression", "runtime.std-data-services"),
        link("StdDatetime", "runtime.std-data-services"),
        link("StdEncoding", "runtime.std-data-services"),
        link("StdEnviron", "runtime.std-hosted-services"),
        link("StdFs", "runtime.std-hosted-services"),
        link("StdGraph", "runtime.std-data-services"),
        link("StdHash", "runtime.std-data-services"),
        link("StdIo", "runtime.std-hosted-services"),
        link("StdJson", "runtime.std-data-services"),
        link("StdLogging", "runtime.std-observability"),
        link("StdMath", "runtime.std-data-services"),
        link("StdRegex", "runtime.std-data-services"),
        link("StdRegistry", "runtime.std-observability"),
        link("StdTelemetryCore", "runtime.std-observability"),
        link("StdTempfile", "runtime.std-hosted-services"),
        link("StdUuid", "runtime.std-data-services"),
        link("StdWeb", "runtime.std-hosted-services"),
        link("SymbolAliases", "module.identity-and-aliases"),
        link("TestRunner", "testing-and-tooling"),
        link("TestingAssertions", "testing-and-tooling"),
        link("ToolchainInstallerManifest", "testing-and-tooling"),
        link("TypeTokensReflection", "types.traits-generics-reflection"),
        link("UnionTypes", "nominal.models-unions-enums"),
        link("UserDefinedDecorators", "decorators.dsl-surfaces"),
        link("ValidatedNewtypes", "nominal.models-unions-enums"),
        link("ValueEnums", "nominal.models-unions-enums"),
        link("VariadicAndSpreadCalls", "call.named-and-variadic"),
        link("WorkspaceMultiPackageProjects", "package.public-boundaries"),
        link("ZeroCloneStarterFlow", "testing-and-tooling"),
    ]
}

/// Build one public-to-feature relation using the shared crosswalk rationale.
fn link(capability_id: &'static str, feature_id: &'static str) -> PublicFeatureLink {
    PublicFeatureLink {
        capability_id,
        feature_id,
        rationale: "The public capability contributes source-observable behavior to this independently probeable contract.",
    }
}

/// Construct temporary feature-to-requirement links whose owning mechanism has not reached local registration yet.
fn migration_bootstrap_feature_requirement_links() -> Vec<FeatureRequirementLink> {
    vec![
        req_link("call.named-and-variadic", "call.argument-binder"),
        req_link("call.named-and-variadic", "call.frames"),
        req_link("call.named-and-variadic", "types.resolved-dispatch"),
        req_link("decorators.dsl-surfaces", "surface.decorator-dispatch"),
        req_link("diagnostics.stable", "diagnostics.source-authority"),
        req_link("diagnostics.stable", "receipts.comparison"),
        req_link("error.result-and-try", "error.result-routing"),
        req_link("error.result-and-try", "diagnostics.source-authority"),
        req_link("generator.expressions", "suspension.continuations"),
        req_link("generator.expressions", "iteration.protocol-dispatch"),
        req_link("generator.functions", "suspension.continuations"),
        req_link("generator.functions", "call.frames"),
        req_link("iteration.protocol-and-adapters", "iteration.protocol-dispatch"),
        req_link("iteration.protocol-and-adapters", "call.frames"),
        req_link("iteration.user-and-fallible", "iteration.protocol-dispatch"),
        req_link("iteration.user-and-fallible", "error.result-routing"),
        req_link("language.aggregates-and-projections", "runtime.aggregate-store"),
        req_link("language.control-flow-complete", "control.normalized-flow"),
        req_link("language.control-flow-complete", "runtime.aggregate-store"),
        req_link("language.match-and-patterns", "patterns.dispatch"),
        req_link("language.match-and-patterns", "nominal.value-model"),
        req_link("language.numeric-complete", "runtime.aggregate-store"),
        req_link("language.strings-and-format", "runtime.scalar-values"),
        req_link("module.identity-and-aliases", "modules.canonical-identity"),
        req_link("nominal.models-unions-enums", "nominal.value-model"),
        req_link("package.public-boundaries", "packages.public-contract"),
        req_link("runtime.std-data-services", "providers.runtime-services"),
        req_link("runtime.std-data-services", "runtime.aggregate-store"),
        req_link("runtime.std-hosted-services", "providers.runtime-services"),
        req_link("runtime.std-hosted-services", "receipts.comparison"),
        req_link("runtime.std-observability", "providers.runtime-services"),
        req_link("testing-and-tooling", "testing.tooling-control-plane"),
        req_link("testing-and-tooling", "receipts.comparison"),
        req_link("types.traits-generics-reflection", "types.resolved-dispatch"),
        req_link("interop.rust-and-c", "unsafe.interop-boundary"),
        req_link("interop.rust-and-c", "packages.public-contract"),
    ]
}

/// Build one local feature-to-private-requirement relation.
pub(crate) fn feature_requirement_link(
    feature_id: &'static str,
    requirement_id: &'static str,
) -> FeatureRequirementLink {
    FeatureRequirementLink {
        feature_id,
        requirement_id,
        rationale: "The private mechanism is required to preserve this source-observable contract without backend-specific rediscovery.",
    }
}

/// Build one temporary bootstrap feature-to-requirement relation through the shared local relation shape.
fn req_link(feature_id: &'static str, requirement_id: &'static str) -> FeatureRequirementLink {
    feature_requirement_link(feature_id, requirement_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn frozen_snapshot_matches_the_v0_5_tag_blob_revision_and_descriptor_count()
    -> Result<(), Box<dyn std::error::Error>> {
        let baseline = checked_v0_5_public_capability_baseline()?;
        let root = registry_workspace_root()?;
        let revision = Command::new("git")
            .args(["-C", root.to_string_lossy().as_ref(), "rev-parse", "v0.5.0^{}"])
            .output()?;
        assert!(revision.status.success());
        assert_eq!(String::from_utf8(revision.stdout)?.trim(), baseline.release.revision);
        let blob = Command::new("git")
            .args([
                "-C",
                root.to_string_lossy().as_ref(),
                "ls-tree",
                "v0.5.0",
                FEATURES_SOURCE_AT_V0_5,
            ])
            .output()?;
        assert!(blob.status.success());
        assert!(String::from_utf8(blob.stdout)?.contains(&baseline.release.source_blob));
        assert_eq!(baseline.release.source_snapshot_path, FROZEN_V0_5_CAPABILITIES_PATH);
        assert!(matches!(
            baseline.release.role,
            ReleaseBaselineRole::MigrationCompatibilityTarget
        ));
        assert!(
            baseline
                .release
                .source_snapshot_path
                .contains("migration_baselines/v0.5.0")
        );
        assert!(baseline.release.retirement_condition.contains("replacement migration"));
        let migration_baseline_readme =
            fs::read_to_string(root.join("src/replacement_compatibility/migration_baselines/README.md"))?;
        assert!(migration_baseline_readme.contains("not a historical stdlib archive"));
        assert_eq!(baseline.capabilities.len(), 67);
        Ok(())
    }

    #[test]
    fn frozen_baseline_ignores_simulated_live_capability_registry_edits() -> Result<(), Box<dyn std::error::Error>> {
        let before = checked_v0_5_public_capability_baseline()?;
        let root = registry_workspace_root()?;
        let mut simulated_live = fs::read(root.join(LIVE_FEATURES_SOURCE))?;
        simulated_live.extend_from_slice(b"\n# simulated post-v0.5 capability registry edit\n");

        assert_ne!(git_blob_id(&simulated_live), before.release.source_blob);
        let after = checked_v0_5_public_capability_baseline()?;
        assert_eq!(before, after);
        Ok(())
    }

    #[test]
    fn validator_rejects_an_unmapped_baseline_capability() -> Result<(), Box<dyn std::error::Error>> {
        let baseline = checked_v0_5_public_capability_baseline()?;
        let mut registry = replacement_compatibility_registry();
        registry.feature_links.retain(|link| link.capability_id != "StdWeb");

        let error = validate_replacement_compatibility_registry(&baseline, &registry)
            .err()
            .ok_or("expected an unmapped baseline capability error")?;
        assert!(error.to_string().contains("StdWeb"));
        Ok(())
    }

    #[test]
    fn validator_rejects_anonymous_or_nonretiring_registration_sources() -> Result<(), Box<dyn std::error::Error>> {
        let baseline = checked_v0_5_public_capability_baseline()?;
        let mut registry = replacement_compatibility_registry();
        let bootstrap = registry
            .registration_sources
            .iter_mut()
            .find(|source| matches!(source.lifecycle, CompatibilityRegistrationLifecycle::MigrationBootstrap))
            .ok_or("missing migration bootstrap source")?;
        bootstrap.retirement_condition = None;
        let local = registry
            .registration_sources
            .iter_mut()
            .find(|source| source.id == "frontend.body-ir.callable-values")
            .ok_or("missing Body-IR callable registration")?;
        local.retirement_condition = Some("mutation fixture".to_string());
        local.feature_ids.push("language.control-flow".to_string());

        let error = validate_replacement_compatibility_registry(&baseline, &registry)
            .err()
            .ok_or("expected registration-source validation errors")?;
        let rendered = error.to_string();
        assert!(rendered.contains("migration bootstrap registration `replacement-compatibility.migration-bootstrap` lacks an explicit retirement condition"));
        assert!(rendered.contains(
            "local implementation registration `frontend.body-ir.callable-values` has a migration retirement condition"
        ));
        assert!(rendered.contains(
            "compatibility feature `language.control-flow` is registered by 2 sources instead of exactly one"
        ));
        Ok(())
    }

    #[test]
    fn closed_out_of_envelope_taxonomy_can_explain_an_unmapped_capability() -> Result<(), Box<dyn std::error::Error>> {
        let baseline = checked_v0_5_public_capability_baseline()?;
        let mut registry = replacement_compatibility_registry();
        registry.feature_links.retain(|link| link.capability_id != "StdWeb");
        registry.out_of_envelope.push(BaselineOutOfEnvelopeRationale {
            capability_id: "StdWeb",
            category: OutOfEnvelopeCategory::HostedProviderBoundary,
            rationale: "The direct profile excludes an external hosted-provider authority boundary.".to_string(),
        });

        validate_replacement_compatibility_registry(&baseline, &registry)?;
        assert_eq!(registry.out_of_envelope[0].category.as_str(), "HostedProviderBoundary");
        Ok(())
    }

    #[test]
    fn validator_rejects_missing_feature_probe_owner_requirement_and_evidence() -> Result<(), Box<dyn std::error::Error>>
    {
        let baseline = checked_v0_5_public_capability_baseline()?;
        let mut registry = replacement_compatibility_registry();
        let feature = registry
            .features
            .iter_mut()
            .find(|feature| feature.id == "call.stored-callables")
            .ok_or("missing stored-callables feature")?;
        feature.probes.clear();
        feature.disposition = CompatibilityDisposition::Unclassified;
        feature.owner_issue = None;
        feature.migration_or_blocker = None;
        feature.evidence.direct_replacement = DirectReplacementOutcome::ExplicitlyRefused;
        feature.evidence.surfaces.replacement_executor.status = EvidenceAnchorStatus::Planned;
        registry
            .requirement_links
            .retain(|link| link.feature_id != "call.stored-callables");

        let error = validate_replacement_compatibility_registry(&baseline, &registry)
            .err()
            .ok_or("expected feature validation errors")?;
        let rendered = error.to_string();
        assert!(rendered.contains("lacks source probes"));
        assert!(rendered.contains("lacks a compatibility disposition"));
        assert!(rendered.contains("lacks an owning issue"));
        assert!(rendered.contains("lacks implementation requirements"));
        assert!(rendered.contains("direct outcome without an observed replacement-executor anchor"));
        Ok(())
    }

    #[test]
    fn validator_rejects_orphaned_requirements_and_unproven_comparison() -> Result<(), Box<dyn std::error::Error>> {
        let baseline = checked_v0_5_public_capability_baseline()?;
        let mut registry = replacement_compatibility_registry();
        registry
            .requirement_links
            .retain(|link| link.requirement_id != "async.runtime");
        let feature = registry
            .features
            .iter_mut()
            .find(|feature| feature.id == "language.control-flow")
            .ok_or("missing control-flow feature")?;
        feature.evidence.independent_comparison = IndependentComparisonState::ComparedMatch;

        let error = validate_replacement_compatibility_registry(&baseline, &registry)
            .err()
            .ok_or("expected requirement/comparison validation errors")?;
        let rendered = error.to_string();
        assert!(rendered.contains("async.runtime` is orphaned"));
        assert!(rendered.contains("comparison without a legacy run"));
        assert!(rendered.contains("comparison without paired evidence"));
        Ok(())
    }

    #[test]
    fn validator_rejects_missing_incoming_links_dangling_anchors_and_unstable_corpus_references()
    -> Result<(), Box<dyn std::error::Error>> {
        let baseline = checked_v0_5_public_capability_baseline()?;
        let mut registry = replacement_compatibility_registry();
        registry
            .feature_links
            .retain(|link| link.feature_id != "call.stored-callables");
        let feature = registry
            .features
            .iter_mut()
            .find(|feature| feature.id == "call.stored-callables")
            .ok_or("missing stored-callables feature")?;
        feature.evidence.surfaces.source_ast.selector = "missing frozen capability selector".to_string();
        feature.evidence.surfaces.parity_corpus = ParityCorpusReference::Planned {
            case_id: "unstructured #987 prose".to_string(),
            owner_issue: 987,
            anchor: planned_anchor(
                EvidenceSurface::ParityCorpus,
                "tests/parity_corpus_tests.rs",
                "fn seed_corpus",
                987,
                "Mutation fixture.",
            ),
        };

        let error = validate_replacement_compatibility_registry(&baseline, &registry)
            .err()
            .ok_or("expected surface validation errors")?;
        let rendered = error.to_string();
        assert!(rendered.contains("lacks an incoming public-capability relation"));
        assert!(rendered.contains("selector `missing frozen capability selector` is dangling"));
        assert!(rendered.contains("unstable #987 planned corpus reference"));
        Ok(())
    }

    #[test]
    fn validator_rejects_unlinked_or_unproven_scoped_comparisons() -> Result<(), Box<dyn std::error::Error>> {
        let baseline = checked_v0_5_public_capability_baseline()?;
        let mut registry = replacement_compatibility_registry();
        let feature = registry
            .features
            .iter_mut()
            .find(|feature| feature.id == "language.numeric-and-scalar")
            .ok_or("missing numeric-and-scalar feature")?;
        let comparison = feature
            .evidence
            .surfaces
            .scoped_comparisons
            .first_mut()
            .ok_or("missing scoped replacement-body-v0-001 comparison")?;
        comparison.case_id = "replacement-body-v0-007".to_string();
        comparison.state = IndependentComparisonState::NonGreenShadowUnavailable;

        let error = validate_replacement_compatibility_registry(&baseline, &registry)
            .err()
            .ok_or("expected scoped comparison validation errors")?;
        let rendered = error.to_string();
        assert!(rendered.contains("scoped comparison for unlinked #987 case `replacement-body-v0-007`"));
        assert!(rendered.contains("paired comparison evidence without a compared state"));
        Ok(())
    }

    #[test]
    fn validator_rejects_completed_comparison_infrastructure_as_outstanding_evidence_owner()
    -> Result<(), Box<dyn std::error::Error>> {
        let baseline = checked_v0_5_public_capability_baseline()?;
        let mut registry = replacement_compatibility_registry();
        let feature = registry
            .features
            .iter_mut()
            .find(|feature| feature.id == "call.stored-callables")
            .ok_or("missing stored-callables feature")?;
        let ComparisonEvidence::Unavailable {
            outstanding_evidence, ..
        } = &mut feature.evidence.surfaces.independent_comparison
        else {
            return Err("stored-callables must begin with unavailable comparison evidence".into());
        };
        *outstanding_evidence = OutstandingComparisonEvidence::Scheduled {
            owner_issue: COMPLETED_COMPARISON_INFRASTRUCTURE_ISSUE,
            note: "Mutation fixture that conflates completed infrastructure with future evidence ownership."
                .to_string(),
        };

        let error = validate_replacement_compatibility_registry(&baseline, &registry)
            .err()
            .ok_or("expected completed-infrastructure ownership validation error")?;
        let rendered = error.to_string();
        assert!(rendered.contains("assigns completed comparison infrastructure #1146 as outstanding evidence owner"));
        assert!(
            rendered.contains(
                "schedules outstanding comparison evidence for #1146, which does not match the feature owner"
            )
        );
        Ok(())
    }

    #[test]
    fn machine_projection_is_named_and_versioned() -> Result<(), Box<dyn std::error::Error>> {
        let baseline = checked_v0_5_public_capability_baseline()?;
        let registry = replacement_compatibility_registry();
        let document: serde_json::Value =
            serde_json::from_str(&render_machine_readable_inventory(&baseline, &registry)?)?;

        assert!(document.is_object());
        assert_eq!(
            document.get("schema_version").and_then(serde_json::Value::as_u64),
            Some(u64::from(REPLACEMENT_COMPATIBILITY_INVENTORY_SCHEMA_VERSION))
        );
        assert!(document.get("baseline").is_some());
        assert!(document.get("registry").is_some());
        Ok(())
    }

    #[test]
    fn committed_projections_match_the_validated_registry() -> Result<(), Box<dyn std::error::Error>> {
        let baseline = checked_v0_5_public_capability_baseline()?;
        let registry = replacement_compatibility_registry();
        let expected_markdown = render_developer_projection(&baseline, &registry)?;
        let expected_json = render_machine_readable_inventory(&baseline, &registry)?;
        let root = registry_workspace_root()?;
        let markdown = fs::read_to_string(
            root.join("workspaces/docs-site/docs/contributing/reference/replacement_compatibility_inventory.md"),
        )?;
        let json = fs::read_to_string(
            root.join("workspaces/docs-site/docs/contributing/reference/replacement_compatibility_inventory.json"),
        )?;

        assert_eq!(markdown, expected_markdown);
        assert_eq!(json, expected_json);
        Ok(())
    }
}
