//! Public harvest models, admission refusals, evidence inputs, and publisher identity.

use std::collections::BTreeSet;

use oven_model::manifest::{
    RustFactArgument, RustFactArtifact, RustFactCompileEnvironment, RustFactEnvironment, RustFactExecutable,
    RustFactLibrary, RustFactLibraryKind, RustFactLinkObject, RustFactOut, RustFactOutput, RustFactRecord,
    RustFactWorkObservation, RustFactWorkRecord, is_sha256_identity,
};
use serde::{Deserialize, Serialize};

use super::super::{OvenLegacyCargoPrepareResult, OvenLegacyCargoSelectedCompilerContext};
use super::tool::{observed_products_are_bound, tool_record_from_observation};
use crate::loaf_bake::OvenLoafPublisherProvenance;

/// The evidence method recorded for facts observed by the compatibility publisher.
pub const HARVEST_EVIDENCE_METHOD: &str = "compatibility publisher observation";

/// Hazard token for a publisher environment carrying RUSTC_BOOTSTRAP.
pub const HARVEST_HAZARD_RUSTC_BOOTSTRAP: &str = "RUSTC_BOOTSTRAP";

/// Hazard token for a compiler identity naming a nightly build.
pub const HARVEST_HAZARD_NIGHTLY_RUSTC: &str = "nightly-rustc";

/// Ambient variables whose presence makes a harvest hazardous.
pub(super) const HARVEST_HAZARD_VARIABLES: &[(&str, &str)] = &[("RUSTC_BOOTSTRAP", HARVEST_HAZARD_RUSTC_BOOTSTRAP)];

/// File name of the proposal inside each package-version-profile directory.
pub const HARVEST_PROPOSAL_FILE: &str = "proposal.json";

/// Directory beneath a proposal that contains committed generated inputs.
pub const HARVEST_OUT_DIRECTORY: &str = "out";

// ============================================================================
// Proposal shape
// ============================================================================

/// `project`: the package the proposal describes, as the registry names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarvestProject {
    /// crates.io package name.
    pub name: String,
    /// Exact published version.
    pub version: String,
}

/// `source`: the registry publication the facts are bound to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarvestSource {
    /// The registry index, without Cargo's `registry+` source-kind prefix.
    pub registry: String,
    /// `sha256:` checksum of the published archive.
    pub checksum: String,
}

/// One candidate `[[rust.facts]]` record plus observations checked before the proposal reaches disk.
///
/// `harvested-from` is derived by admission from `evidence.receipt`. Complete link/tool observations become typed
/// records; portable environment values become typed declarations, while unresolved paths and incomplete publisher
/// work remain refusals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarvestFact {
    /// Exact compiler identity the answers were observed under (`rustc -vV` first line).
    pub toolchain: String,
    /// Target triple.
    pub target: String,
    /// `release` or `debug`.
    pub profile: String,
    /// Complete enabled Cargo feature set, sorted and unique.
    pub features: Vec<String>,
    /// `--cfg` answers the script emitted, sorted and unique; empty is a stated fact.
    pub cfg: Vec<String>,
    /// Retained generated inputs: `name` is the member path, `path` is `out/<name>` relative to the proposal.
    pub out: Vec<RustFactOut>,
    /// Exact compile-time environment emitted by the script, with generated paths relative to `OUT_DIR`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environment: Vec<RustFactCompileEnvironment>,
    /// Script environment values that losslessly name retained owner-relative inputs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environment_inputs: Vec<HarvestEnvironmentInput>,
    /// Publisher-only native-link observations awaiting fail-closed typed conversion.
    #[serde(skip)]
    pub link_observations: Vec<HarvestLinkObservation>,
    /// Publisher-only compiler/tool observations awaiting fail-closed typed conversion.
    #[serde(skip)]
    pub tool_observations: Vec<HarvestToolObservation>,
    /// Fully admitted native-link records derived from the raw observations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub link: Vec<oven_model::manifest::RustFactLink>,
    /// Fully admitted generator records derived from the raw observations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool: Vec<oven_model::manifest::RustFactTool>,
}

/// A script environment value rebound to one retained product rather than preserving an ambient scalar or path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarvestEnvironmentInput {
    /// Environment variable name observed from Cargo's build-script output.
    pub name: String,
    /// Path below the retained output owner, independent of the publisher staging root.
    pub owner_relative_path: String,
    /// Byte identity of that exact retained member.
    pub digest: String,
}

/// One retained product named by a raw link or tool observation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarvestObservedProduct {
    /// Path below the retained output owner.
    pub owner_relative_path: String,
    /// Byte identity of the product.
    pub digest: String,
}

/// Native-link evidence that either converts completely or is retained on a typed refusal.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarvestLinkObservation {
    /// Cargo link-name/kind directives, sorted and unique.
    pub libraries: Vec<String>,
    /// Search paths rebound to the retained output owner.
    pub search_paths: Vec<HarvestLinkSearchPath>,
    /// Identity of the complete retained output tree.
    pub output_tree_digest: String,
    /// Identities of every retained product member.
    pub products: Vec<HarvestObservedProduct>,
    /// Stable producer name, when capture proved one complete native invocation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Exact native compiler identity, when capture proved it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<RustFactExecutable>,
    /// Sorted explicit object compilations, when capture proved the complete native work.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub objects: Vec<RustFactLinkObject>,
    /// Explicit invocation environment.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environment: Vec<RustFactEnvironment>,
    /// Complete source closure inside the checksummed crate source.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<RustFactArtifact>,
    /// Logical library contract, when capture proved it unambiguously.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library: Option<RustFactLibrary>,
    /// Precise reason the raw native invocations could not become typed publisher work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversion_refusal: Option<String>,
}

/// One native-link search path whose ownership was proven below the retained output root.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarvestLinkSearchPath {
    /// Cargo search-path kind before `=`, or `all` for an unqualified path.
    pub kind: String,
    /// Path below the retained output owner; `.` names its root.
    pub owner_relative_path: String,
}

/// Compiler/tool evidence that either converts completely or is retained on a typed refusal.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarvestToolObservation {
    /// Host/target domain Cargo assigned to the script invocation.
    pub target_context: String,
    /// Target argument of the exact compiler probe.
    pub rustc_target: String,
    /// Canonical identity of the probe invocation and its declared environment.
    pub probe_digest: String,
    /// Byte identity of the bounded compiler closure containing the exact probe executable.
    pub executable_identity: String,
    /// Identity of the complete retained output tree.
    pub output_tree_digest: String,
    /// Identities of every retained generated product member.
    pub products: Vec<HarvestObservedProduct>,
    /// Stable producer name, when capture proved one complete tool invocation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Exact generator identity, when capture proved it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<RustFactExecutable>,
    /// Ordered portable invocation arguments.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arguments: Vec<RustFactArgument>,
    /// Explicit invocation environment.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environment: Vec<RustFactEnvironment>,
    /// Complete declared input closure inside its immutable owner.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<RustFactArtifact>,
    /// Complete logical output contract; product digests remain asset-side.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<RustFactOutput>,
}

/// Effect classes used by the generated closure inventory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HarvestEffectClass {
    /// The selected binding explicitly observes no build effect.
    Empty,
    /// Compiler cfg answers were observed.
    Cfg,
    /// Retained generated inputs were observed.
    Out,
    /// Compile-time environment values were observed.
    EnvironmentInput,
    /// Publisher-only native-link work was observed.
    Link,
    /// Publisher-only tool work was observed.
    Tool,
}

impl HarvestFact {
    /// Sorted effect classes for closure-completeness checks, including an explicit empty class.
    pub fn effect_classes(&self) -> Vec<HarvestEffectClass> {
        let mut effects = Vec::new();
        if !self.cfg.is_empty() {
            effects.push(HarvestEffectClass::Cfg);
        }
        if !self.environment.is_empty() || !self.environment_inputs.is_empty() {
            effects.push(HarvestEffectClass::EnvironmentInput);
        }
        if !self.link_observations.is_empty() || !self.link.is_empty() {
            effects.push(HarvestEffectClass::Link);
        }
        if !self.out.is_empty() {
            effects.push(HarvestEffectClass::Out);
        }
        if !self.tool_observations.is_empty() || !self.tool.is_empty() {
            effects.push(HarvestEffectClass::Tool);
        }
        if effects.is_empty() {
            effects.push(HarvestEffectClass::Empty);
        }
        effects
    }
}

/// `rust`: the fact list, which a proposal keeps to exactly one entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarvestRustFacts {
    /// Exactly one record per proposal.
    pub facts: Vec<HarvestFact>,
}

/// `evidence`: what the observation ran under. Admission stores `receipt` as `harvested-from`, refuses a proposal
/// whose `hazards` name anything, and records the rest verbatim on the event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarvestEvidence {
    /// Always [`HARVEST_EVIDENCE_METHOD`].
    pub method: String,
    /// `sha256:` identity of the compatibility receipt the publisher ran under; absent when the caller had no
    /// identity of that shape, since admission accepts no other spelling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<String>,
    /// `sha256:` identity of the bounded compiler/sysroot closure that compiled the observation.
    pub rustc_identity: String,
    /// The publisher host triple.
    pub host: String,
    /// Publisher-environment facts that make the harvest untrustworthy, sorted and unique; empty when clean.
    ///
    /// Exactly these tokens: [`HARVEST_HAZARD_RUSTC_BOOTSTRAP`] when that variable was set,
    /// [`HARVEST_HAZARD_NIGHTLY_RUSTC`] when the compiler identity names a nightly. Admission refuses a proposal
    /// that names any; the harvest records and never refuses on them. The publisher Cargo's own version is
    /// provenance in `cargo_version`, not a hazard.
    pub hazards: Vec<String>,
    /// `cargo --version` as the publisher observed it.
    pub cargo_version: String,
    /// `sha256:` digest of the publisher's `Cargo.lock`.
    pub cargo_lock_digest: String,
    /// `sha256:` digest of the publisher's `Cargo.toml`.
    pub cargo_manifest_digest: String,
    /// Sorted, unique digests of the compiler probes whose answers are the fact's `cfg` list; empty when the build
    /// script probed nothing.
    ///
    /// A probe digest covers the probe's exact invocation, so it can differ between units and runs that answered the
    /// same. It is evidence of what was asked, never part of the fact admission compares.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compiler_probes: Vec<String>,
}

/// One harvest proposal, as `incan-pub add-fact` admits it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarvestProposal {
    pub project: HarvestProject,
    pub source: HarvestSource,
    pub rust: HarvestRustFacts,
    pub evidence: HarvestEvidence,
    /// The publish note admission records when it creates the package's record from this proposal; absent when
    /// the harvesting command knew no checkout to name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// Where the retained `OUT_DIR` tree of this fact lives, relative to the retention root the writer is given.
    ///
    /// Never serialized: it is a publisher-local coordinate, and the proposal names its inputs by digest.
    #[serde(skip)]
    pub out_relative_root: Option<String>,
}

/// Why a canonical proposal cannot yet become an admitted manifest fact.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HarvestAdmissionRefusal {
    /// A proposal must contain exactly one binding.
    #[error("harvest proposal must contain exactly one fact")]
    FactCount,
    /// At least one retained environment input has no admitted typed constant representation.
    #[error("harvest proposal contains unresolved environment-input observations")]
    UnresolvedEnvironmentInputs,
    /// At least one native-link observation is incomplete, invalid, or names an unbound byte.
    #[error("harvest proposal contains unresolved native-link observations")]
    UnresolvedLinkObservations,
    /// At least one tool observation is incomplete, invalid, or names an unbound byte.
    #[error("harvest proposal contains unresolved tool observations")]
    UnresolvedToolObservations,
    /// One generated path was observed with different bytes in retained `out` and typed publisher work.
    #[error("harvest proposal contains conflicting generated-output observations")]
    ConflictingObservations,
}

impl HarvestProposal {
    /// Convert a complete proposal into the registry record shape through the model-owned fail-closed boundary.
    ///
    /// Native-link and tool observations are admitted only when every executable, declared input, argument, output
    /// contract, and observed byte identity is complete. Generated inputs remain `out` members copied beside the
    /// proposal; declared crate sources and executable bytes remain owner-relative under their immutable owners.
    pub fn admitted_record(&self) -> Result<RustFactRecord, HarvestAdmissionRefusal> {
        let [fact] = self.rust.facts.as_slice() else {
            return Err(HarvestAdmissionRefusal::FactCount);
        };
        if !fact.environment_inputs.is_empty() {
            return Err(HarvestAdmissionRefusal::UnresolvedEnvironmentInputs);
        }
        let mut out = fact.out.clone();
        let mut link = fact.link.clone();
        for observation in &fact.link_observations {
            remove_observed_products(
                &mut out,
                &observation.products,
                HarvestAdmissionRefusal::UnresolvedLinkObservations,
            )?;
            let record = link_record_from_observation(observation, &fact.target)?;
            link.push(record);
        }
        let mut tool = fact.tool.clone();
        for observation in &fact.tool_observations {
            remove_observed_products(
                &mut out,
                &observation.products,
                HarvestAdmissionRefusal::UnresolvedToolObservations,
            )?;
            let record = tool_record_from_observation(observation, &fact.target)?;
            tool.push(record);
        }
        link.sort_by(|left, right| left.name.cmp(&right.name));
        tool.sort_by(|left, right| left.name.cmp(&right.name));
        let mut producer_names = BTreeSet::new();
        if link.iter().any(|record| !producer_names.insert(record.name.as_str())) {
            return Err(HarvestAdmissionRefusal::UnresolvedLinkObservations);
        }
        if tool.iter().any(|record| !producer_names.insert(record.name.as_str())) {
            return Err(HarvestAdmissionRefusal::UnresolvedToolObservations);
        }
        Ok(RustFactRecord {
            toolchain: fact.toolchain.clone(),
            target: fact.target.clone(),
            profile: fact.profile.clone(),
            features: fact.features.clone(),
            cfg: fact.cfg.clone(),
            out,
            environment: fact.environment.clone(),
            link,
            tool,
            harvested_from: self.evidence.receipt.clone(),
        })
    }
}

/// Remove asset-side producer products from the generated inputs copied beside a proposal.
fn remove_observed_products(
    out: &mut Vec<RustFactOut>,
    products: &[HarvestObservedProduct],
    refusal: HarvestAdmissionRefusal,
) -> Result<(), HarvestAdmissionRefusal> {
    let product_names = products
        .iter()
        .map(|product| product.owner_relative_path.as_str())
        .collect::<BTreeSet<_>>();
    if product_names.len() != products.len() {
        return Err(refusal);
    }
    for product in products {
        let Some(member) = out.iter().find(|member| member.name == product.owner_relative_path) else {
            return Err(refusal);
        };
        if member.digest != product.digest {
            return Err(HarvestAdmissionRefusal::ConflictingObservations);
        }
    }
    out.retain(|member| !product_names.contains(member.name.as_str()));
    Ok(())
}

/// Convert one raw link observation after proving that all product evidence is digest-bound.
pub(super) fn link_record_from_observation(
    observation: &HarvestLinkObservation,
    target: &str,
) -> Result<oven_model::manifest::RustFactLink, HarvestAdmissionRefusal> {
    if !observed_products_are_bound(&observation.output_tree_digest, &observation.products) {
        return Err(HarvestAdmissionRefusal::UnresolvedLinkObservations);
    }
    let work = RustFactWorkObservation {
        role: oven_model::manifest::RustFactProducerRole::Link,
        name: observation.name.clone().unwrap_or_default(),
        target: target.to_string(),
        executable: observation.executable.clone(),
        objects: observation.objects.clone(),
        arguments: Vec::new(),
        environment: observation.environment.clone(),
        inputs: observation.sources.clone(),
        outputs: Vec::new(),
        library: observation.library.clone(),
    };
    match RustFactWorkRecord::try_from_observation(work) {
        Ok(RustFactWorkRecord::Link(link))
            if observation.libraries
                == [format!(
                    "{}={}",
                    match link.library.kind {
                        RustFactLibraryKind::Static => "static",
                        RustFactLibraryKind::Dynamic => "dylib",
                    },
                    link.library.name
                )] =>
        {
            Ok(link)
        }
        Ok(RustFactWorkRecord::Link(_)) | Ok(RustFactWorkRecord::Tool(_)) | Err(_) => {
            Err(HarvestAdmissionRefusal::UnresolvedLinkObservations)
        }
    }
}

// ============================================================================
// Refusals
// ============================================================================

/// Why one captured unit was not proposed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HarvestRefusalReason {
    /// The unit is not bound to a staged registry source (a path, git or transport-root unit).
    NotRegistryBacked,
    /// The unit is the run-custom-build execution node; its facts are proposed on its consumer.
    BuildScriptUnit,
    /// The script linked libraries but its typed `link` declaration was incomplete or invalid.
    LinkedLibraries,
    /// The script emitted link search paths but its typed `link` declaration was incomplete or invalid.
    LinkedPaths,
    /// The script ran compiler/tool work but its typed `tool` declaration was incomplete or invalid.
    ToolProbes,
    /// The script emitted a compile environment value that cannot be represented portably as literal text or an
    /// `OUT_DIR`-relative path.
    EnvironmentObserved,
    /// A recognized script-emitted constant disagrees with the value derived from the selected binding.
    BindingDerivedEnvironmentMismatch,
    /// The script's `OUT_DIR` was never retained, so its members cannot be named by digest.
    OutputNotRetained,
    /// The unit compiled for a platform other than the captured target, which the reader binds every record to.
    TargetMismatch,
    /// More than one build-script edge feeds the unit; the observation is not one script's answer.
    MultipleBuildScriptEdges,
    /// Two units of the same package, version and selection observed different facts.
    ConflictingObservations,
    /// Another binding of this package refused, so the whole package is withheld from the drop.
    PackageHasRefusedBinding,
    /// The staged registry source names a checksum that is not a SHA-256 identity, so no record can bind it.
    MalformedChecksum,
    /// A retained `OUT_DIR` member has no plain relative path, no sha256 digest, or is inventoried twice.
    MalformedOutput,
}

/// Portable raw evidence retained on a refusal for incomplete or invalid publisher work.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarvestRefusalObservations {
    /// Owner-relative environment inputs and their byte identities.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environment: Vec<HarvestEnvironmentInput>,
    /// Native-link observations and retained product identities.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub link: Vec<HarvestLinkObservation>,
    /// Tool-probe observations, executable identity, and retained product identities.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool: Vec<HarvestToolObservation>,
}

impl HarvestRefusalObservations {
    /// Whether the refusal carries no portable raw observation beyond its typed reason and path-free detail.
    fn is_empty(&self) -> bool {
        self.environment.is_empty() && self.link.is_empty() && self.tool.is_empty()
    }
}

/// One unit the harvest declined to propose, named so a reviewer can see what the closure still lacks.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarvestRefusal {
    /// Package name.
    pub package: String,
    /// Exact version.
    pub version: String,
    /// Typed reason.
    pub reason: HarvestRefusalReason,
    /// Short, path-free detail: the names involved, never their values.
    pub detail: String,
    /// Portable observations that prove which declared facts or byte bindings were incomplete.
    #[serde(default, skip_serializing_if = "HarvestRefusalObservations::is_empty")]
    pub observations: HarvestRefusalObservations,
}

/// Everything one harvest produced: proposals and refusals, each in deterministic order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HarvestReport {
    /// Proposals ordered by package, version, then selection.
    pub proposals: Vec<HarvestProposal>,
    /// Refusals ordered by package, version, reason, detail.
    pub refusals: Vec<HarvestRefusal>,
    /// The hazard tokens every proposal of this harvest carries (see [`HarvestEvidence::hazards`]); recorded
    /// here too so a harvest that proposed nothing still says what it ran under.
    pub hazards: Vec<String>,
    /// The profile this harvest observed; names the refusal list on disk.
    pub profile: String,
}

/// The publisher facts a proposal records as evidence, taken from the preparation that produced the capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarvestEvidenceInputs {
    /// `sha256:` identity of the compatibility receipt the publisher ran under, when the caller had one.
    pub receipt: Option<String>,
    /// `sha256:` identity of the bounded compiler/sysroot closure, as the release Toolchain owner names it.
    pub rustc_identity: String,
    /// Hazard tokens the publisher environment contributed; see [`ambient_harvest_hazards`]. The nightly tokens are
    /// derived from the capture and `cargo_version` by the harvest itself.
    pub ambient_hazards: Vec<String>,
    /// Cargo's version string.
    pub cargo_version: String,
    /// Digest of the publisher `Cargo.lock`.
    pub cargo_lock_digest: String,
    /// Digest of the publisher `Cargo.toml`.
    pub cargo_manifest_digest: String,
    /// The publish note every proposal of this harvest carries, when the command knew a checkout to name.
    pub notes: Option<String>,
}

/// The publisher-side identities every harvest records beside what one preparation observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarvestPublisherIdentity {
    /// The compatibility receipt identity the publisher ran under; kept only when it is a `sha256:` identity.
    pub receipt: Option<String>,
    /// `sha256:` identity of the bounded compiler/sysroot closure that compiled the observation.
    pub rustc_identity: String,
    /// Hazard tokens the publisher environment contributed.
    pub ambient_hazards: Vec<String>,
    /// The publish note, when the command knew a checkout to name.
    pub notes: Option<String>,
}

impl HarvestPublisherIdentity {
    /// Identity under a receipt whose identity is kept only in the one spelling admission accepts.
    pub fn new(receipt: &str, rustc_identity: &str, ambient_hazards: Vec<String>, notes: Option<String>) -> Self {
        Self {
            receipt: is_sha256_identity(receipt).then(|| receipt.to_string()),
            rustc_identity: rustc_identity.to_string(),
            ambient_hazards,
            notes,
        }
    }
}

impl HarvestEvidenceInputs {
    /// Evidence for the capture one `prepare_direct_rustc_plan` call produced.
    pub fn from_prepare_result(prepared: &OvenLegacyCargoPrepareResult, publisher: HarvestPublisherIdentity) -> Self {
        Self {
            receipt: publisher.receipt,
            rustc_identity: publisher.rustc_identity,
            ambient_hazards: publisher.ambient_hazards,
            cargo_version: prepared.cargo_version.clone(),
            cargo_lock_digest: prepared.cargo_lock_digest.clone(),
            cargo_manifest_digest: prepared.cargo_manifest_digest.clone(),
            notes: publisher.notes,
        }
    }

    /// Evidence for the capture one Loaf publication produced.
    pub fn from_loaf_publisher(provenance: &OvenLoafPublisherProvenance, publisher: HarvestPublisherIdentity) -> Self {
        Self {
            receipt: publisher.receipt,
            rustc_identity: publisher.rustc_identity,
            ambient_hazards: publisher.ambient_hazards,
            cargo_version: provenance.cargo_version.clone(),
            cargo_lock_digest: provenance.cargo_lock_digest.clone(),
            cargo_manifest_digest: provenance.cargo_manifest_digest.clone(),
            notes: publisher.notes,
        }
    }
}

/// The hazard tokens this process's environment contributes, sorted.
///
/// The compatibility publisher runs Cargo as a child of the harvesting process and clears only Cargo's own
/// variables, so what this process carries is what the scripts saw. A publisher that finds a hazard must still
/// record the observation; the proposal then says so and admission refuses it.
pub fn ambient_harvest_hazards() -> Vec<String> {
    let mut hazards = HARVEST_HAZARD_VARIABLES
        .iter()
        .filter(|(name, _)| std::env::var_os(name).is_some())
        .map(|(_, token)| (*token).to_string())
        .collect::<Vec<_>>();
    hazards.sort();
    hazards
}

/// The publish note a harvest records for a checkout at `head`, its full or abbreviated commit id.
pub fn harvest_notes_for_checkout(head: &str) -> String {
    let short = head.trim().chars().take(7).collect::<String>();
    format!("harvested from the release capture at {short}")
}

/// Whether a tool identity string (`rustc -vV` / `cargo --version` first line) names a nightly build.
fn names_nightly(identity: &str) -> bool {
    identity.contains("nightly")
}

/// The complete hazard list for one harvest: the ambient tokens plus what the capture says of its compiler.
pub(super) fn harvest_hazards(
    compiler: &OvenLegacyCargoSelectedCompilerContext,
    evidence: &HarvestEvidenceInputs,
) -> Vec<String> {
    let mut hazards = evidence.ambient_hazards.clone();
    if names_nightly(&compiler.toolchain) || names_nightly(&compiler.rustc_identity) {
        hazards.push(HARVEST_HAZARD_NIGHTLY_RUSTC.to_string());
    }
    hazards.sort();
    hazards.dedup();
    hazards
}
