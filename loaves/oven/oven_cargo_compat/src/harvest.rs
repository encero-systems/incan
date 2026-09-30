//! Harvest: what the compatibility publisher observed for each registry unit, written as incan.pub proposals.
//!
//! RFC 119 makes adoption a harvest, not a hand edit: a crates.io package gains its declared build facts by running
//! one coordinated Cargo build for the selected closure on the publisher's machine and reading Cargo's build-script
//! output records into canonical proposals. The capture that build leaves behind
//! (`OvenLegacyCargoSelectedUnitCapture`) is the observation; this module turns it into one proposal per registry
//! package, version and exact selection, and one refusal per unit whose observation cannot be proven or retained.
//! Admitting a proposal is `incan-pub add-fact`'s job, in a separate, reviewable step; nothing here writes into a
//! registry.
//!
//! Every fact proposed here is one `LoafRegistryAuthority::resolve` will later compare with a fresh observation
//! (`check_observation`), so the shape emitted must be the shape the reader compares: `features` and `cfg` sorted and
//! unique, `out.name` exactly the retained member path, `out.path` relative to the proposal file.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use oven_model::loaf_registry::canonical_checksum;
use oven_model::manifest::{
    RustFactArgument, RustFactArtifact, RustFactArtifactKind, RustFactEnvironment, RustFactExecutable, RustFactLibrary,
    RustFactLibraryKind, RustFactLinkObject, RustFactOut, RustFactOutput, RustFactRecord, RustFactWorkObservation,
    RustFactWorkRecord, is_sha256_identity,
};
use serde::{Deserialize, Serialize};

use super::loaf_bake::OvenLoafPublisherProvenance;
use super::{
    OvenLegacyCargoBuildScriptFacts, OvenLegacyCargoError, OvenLegacyCargoPrepareResult,
    OvenLegacyCargoSelectedCompilerContext, OvenLegacyCargoSelectedUnit, OvenLegacyCargoSelectedUnitCapture,
    digest_bytes, regular_file_bytes,
};

/// The `evidence.method` every proposal records: the facts come from watching Cargo, not from reading a manifest.
pub const HARVEST_EVIDENCE_METHOD: &str = "compatibility publisher observation";

/// Hazard token: the `RUSTC_BOOTSTRAP` variable was set in the publisher environment, which lets a stable compiler
/// answer as nightly, so a script's probe under it (`proc-macro2`'s `proc_macro_span`) is not a fact of the
/// toolchain.
pub const HARVEST_HAZARD_RUSTC_BOOTSTRAP: &str = "RUSTC_BOOTSTRAP";

/// Hazard token: the compiler identity the facts bind names a nightly build.
///
/// The Cargo that drove the observation is not a hazard: it compiles nothing, and the compatibility publisher
/// needs a nightly Cargo for `--unit-graph` by design. Its version is provenance and travels verbatim in
/// `cargo_version`.
pub const HARVEST_HAZARD_NIGHTLY_RUSTC: &str = "nightly-rustc";

/// Ambient variables whose presence in the publisher environment is a hazard, with the token each records.
const HARVEST_HAZARD_VARIABLES: &[(&str, &str)] = &[("RUSTC_BOOTSTRAP", HARVEST_HAZARD_RUSTC_BOOTSTRAP)];

/// File name of the refusal list `write_harvest_report` writes beside the proposal directories for one profile.
///
/// A harvest is one profile's observation; a release bake writes its debug and release harvests into one
/// directory, so each profile keeps its own list rather than contending for one file.
pub fn harvest_refusals_file_name(profile: &str) -> String {
    format!("refusals-{profile}.json")
}

/// File name of the proposal inside each `<name>-<version>-<profile>` directory.
pub const HARVEST_PROPOSAL_FILE: &str = "proposal.json";

/// Directory, relative to the proposal, under which committed generated inputs are written.
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
/// records; unresolved environment values and incomplete publisher work remain refusals.
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
    /// Retained environment-to-input bindings were observed.
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
        if !self.environment_inputs.is_empty() {
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
fn link_record_from_observation(
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

/// Convert one raw tool observation after proving executable, invocation, and product byte identities.
fn tool_record_from_observation(
    observation: &HarvestToolObservation,
    target: &str,
) -> Result<oven_model::manifest::RustFactTool, HarvestAdmissionRefusal> {
    if !is_sha256_identity(&observation.probe_digest)
        || !is_sha256_identity(&observation.executable_identity)
        || !observed_products_are_bound(&observation.output_tree_digest, &observation.products)
        || !tool_products_match_outputs(&observation.products, &observation.outputs)
        || observation
            .executable
            .as_ref()
            .is_none_or(|executable| executable.digest != observation.executable_identity)
    {
        return Err(HarvestAdmissionRefusal::UnresolvedToolObservations);
    }
    let work = RustFactWorkObservation {
        role: oven_model::manifest::RustFactProducerRole::Tool,
        name: observation.name.clone().unwrap_or_default(),
        target: target.to_string(),
        executable: observation.executable.clone(),
        objects: Vec::new(),
        arguments: observation.arguments.clone(),
        environment: observation.environment.clone(),
        inputs: observation.inputs.clone(),
        outputs: observation.outputs.clone(),
        library: None,
    };
    match RustFactWorkRecord::try_from_observation(work) {
        Ok(RustFactWorkRecord::Tool(tool)) => Ok(tool),
        Ok(RustFactWorkRecord::Link(_)) | Err(_) => Err(HarvestAdmissionRefusal::UnresolvedToolObservations),
    }
}

/// Whether the asset-side product inventory exactly satisfies the logical tool output contract.
fn tool_products_match_outputs(products: &[HarvestObservedProduct], outputs: &[RustFactOutput]) -> bool {
    outputs.iter().all(|output| {
        products.iter().any(|product| match output.kind {
            RustFactArtifactKind::File => product.owner_relative_path == output.path,
            RustFactArtifactKind::Tree => Path::new(&product.owner_relative_path).starts_with(&output.path),
        })
    }) && products.iter().all(|product| {
        outputs.iter().any(|output| match output.kind {
            RustFactArtifactKind::File => product.owner_relative_path == output.path,
            RustFactArtifactKind::Tree => Path::new(&product.owner_relative_path).starts_with(&output.path),
        })
    })
}

/// Whether every observed product and its complete tree carry canonical byte identities.
fn observed_products_are_bound(tree_digest: &str, products: &[HarvestObservedProduct]) -> bool {
    is_sha256_identity(tree_digest)
        && products
            .iter()
            .all(|product| safe_relative(&product.owner_relative_path).is_some() && is_sha256_identity(&product.digest))
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
    /// The script emitted `rustc-env` values, which Cargo set on the consumer's compilation and no record key
    /// carries.
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
fn harvest_hazards(compiler: &OvenLegacyCargoSelectedCompilerContext, evidence: &HarvestEvidenceInputs) -> Vec<String> {
    let mut hazards = evidence.ambient_hazards.clone();
    if names_nightly(&compiler.toolchain) || names_nightly(&compiler.rustc_identity) {
        hazards.push(HARVEST_HAZARD_NIGHTLY_RUSTC.to_string());
    }
    hazards.sort();
    hazards.dedup();
    hazards
}

// ============================================================================
// Harvesting a capture
// ============================================================================

/// The observation of one registry unit before it is folded with its equals.
struct Observation {
    fact: HarvestFact,
    source: HarvestSource,
    out_relative_root: Option<String>,
    /// Digests of the compiler probes whose only answer is the fact's `cfg` list; evidence, never part of the fact.
    compiler_probes: Vec<String>,
}

/// Turn one capture into proposals for every immediately admissible registry unit and refusals for the rest.
///
/// A unit is harvestable when it is registry-backed, compiled for the captured target, and has at most one retained
/// build-script observation. Binding-derived constants need no new fact. Before writing,
/// [`HarvestProposal::admitted_record`] proves that the candidate declares everything its script did; candidates with
/// unresolved environment/link/tool observations become typed refusals that retain those observations and their byte
/// identities. The proposal's `out` entries name every retained member by path and digest, so the reader's
/// `check_observation` compares equal on the next capture. No package can occur in both proposal and refusal channels
/// for one report.
///
/// The capture must carry its compiler context: without it no record can bind a toolchain or target, so that is an
/// error rather than a refusal of every unit.
pub fn harvest_registry_units(
    capture: &OvenLegacyCargoSelectedUnitCapture,
    evidence: &HarvestEvidenceInputs,
    profile: &str,
) -> Result<HarvestReport, OvenLegacyCargoError> {
    let compiler = capture.compiler.as_ref().ok_or_else(|| {
        OvenLegacyCargoError::Plan("harvest requires a capture with its compiler selection".to_string())
    })?;
    if !matches!(profile, "release" | "debug") {
        return Err(OvenLegacyCargoError::Plan(format!(
            "harvest profile must be `release` or `debug`, not `{profile}`"
        )));
    }
    let hazards = harvest_hazards(compiler, evidence);
    let mut refusals = BTreeSet::new();
    let mut observations: BTreeMap<(String, String, Vec<String>), Vec<Observation>> = BTreeMap::new();

    // ---- Classify every registry binding: refuse, or record its observation ----
    for unit in &capture.units {
        if is_build_script_unit(unit) {
            continue;
        }
        match observe_unit(capture, compiler, unit, profile, &evidence.rustc_identity) {
            Ok(observation) => {
                let key = (
                    unit.package.clone(),
                    unit.package_version.clone(),
                    observation.fact.features.clone(),
                );
                observations.entry(key).or_default().push(observation);
            }
            Err(refusal) => {
                refusals.insert(refusal);
            }
        }
    }

    // ---- Fold equal observations of one selection; refuse a selection whose observations disagree ----
    let mut proposals = Vec::new();
    for ((package, version, _), mut group) in observations {
        let Some(first) = group.pop() else {
            continue;
        };
        if group
            .iter()
            .any(|other| other.fact != first.fact || other.source != first.source)
        {
            let refusal_observations = raw_refusal_observations(
                std::iter::once(&first.fact).chain(group.iter().map(|observation| &observation.fact)),
            );
            refusals.insert(HarvestRefusal {
                package,
                version,
                reason: HarvestRefusalReason::ConflictingObservations,
                detail: format!("{} units of one selection observed different facts", group.len() + 1),
                observations: refusal_observations,
            });
            continue;
        }
        let compiler_probes = first
            .compiler_probes
            .iter()
            .chain(group.iter().flat_map(|observation| observation.compiler_probes.iter()))
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut proposal = HarvestProposal {
            project: HarvestProject { name: package, version },
            source: first.source,
            rust: HarvestRustFacts {
                facts: vec![first.fact],
            },
            evidence: HarvestEvidence {
                method: HARVEST_EVIDENCE_METHOD.to_string(),
                receipt: evidence.receipt.clone(),
                rustc_identity: evidence.rustc_identity.clone(),
                host: compiler.host.clone(),
                hazards: hazards.clone(),
                cargo_version: evidence.cargo_version.clone(),
                cargo_lock_digest: evidence.cargo_lock_digest.clone(),
                cargo_manifest_digest: evidence.cargo_manifest_digest.clone(),
                compiler_probes,
            },
            notes: evidence.notes.clone(),
            out_relative_root: first.out_relative_root,
        };
        match proposal.admitted_record() {
            Ok(record) => {
                let [fact] = proposal.rust.facts.as_mut_slice() else {
                    return Err(OvenLegacyCargoError::Plan(
                        "admitted harvest proposal lost its sole fact".to_string(),
                    ));
                };
                fact.out = record.out;
                fact.link = record.link;
                fact.tool = record.tool;
                fact.link_observations.clear();
                fact.tool_observations.clear();
                proposals.push(proposal);
            }
            Err(reason) => {
                refusals.insert(admission_refusal(&proposal, reason));
            }
        }
    }

    // ---- A registry drop never names one package in both channels ----
    let refused_packages = refusals
        .iter()
        .map(|refusal| refusal.package.clone())
        .collect::<BTreeSet<_>>();
    let mut held_back = Vec::new();
    proposals.retain(|proposal| {
        if refused_packages.contains(&proposal.project.name) {
            held_back.push((proposal.project.name.clone(), proposal.project.version.clone()));
            false
        } else {
            true
        }
    });
    for (package, version) in held_back {
        refusals.insert(HarvestRefusal {
            package,
            version,
            reason: HarvestRefusalReason::PackageHasRefusedBinding,
            detail: "another binding of this package was refused".to_string(),
            observations: HarvestRefusalObservations::default(),
        });
    }
    Ok(HarvestReport {
        proposals,
        refusals: refusals.into_iter().collect(),
        hazards,
        profile: profile.to_string(),
    })
}

/// Convert an in-memory admission refusal into the complete on-disk refusal evidence the registry can inspect.
fn admission_refusal(proposal: &HarvestProposal, refusal: HarvestAdmissionRefusal) -> HarvestRefusal {
    let observations = raw_refusal_observations(proposal.rust.facts.iter());
    let (reason, detail) = match refusal {
        HarvestAdmissionRefusal::FactCount => (
            HarvestRefusalReason::ConflictingObservations,
            "proposal did not contain exactly one fact".to_string(),
        ),
        HarvestAdmissionRefusal::UnresolvedEnvironmentInputs => {
            let names = observations
                .environment
                .iter()
                .map(|input| input.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            (HarvestRefusalReason::EnvironmentObserved, names)
        }
        HarvestAdmissionRefusal::UnresolvedLinkObservations => {
            let libraries = observations
                .link
                .iter()
                .flat_map(|observation| observation.libraries.iter().map(String::as_str))
                .collect::<Vec<_>>();
            if libraries.is_empty() {
                let paths = observations
                    .link
                    .iter()
                    .map(|observation| observation.search_paths.len())
                    .sum::<usize>();
                (
                    HarvestRefusalReason::LinkedPaths,
                    format!("{paths} link search path(s)"),
                )
            } else {
                (HarvestRefusalReason::LinkedLibraries, libraries.join(", "))
            }
        }
        HarvestAdmissionRefusal::UnresolvedToolObservations => (
            HarvestRefusalReason::ToolProbes,
            format!("{} compiler probe(s)", observations.tool.len()),
        ),
        HarvestAdmissionRefusal::ConflictingObservations => (
            HarvestRefusalReason::ConflictingObservations,
            "typed publisher output disagrees with retained out bytes".to_string(),
        ),
    };
    HarvestRefusal {
        package: proposal.project.name.clone(),
        version: proposal.project.version.clone(),
        reason,
        detail,
        observations,
    }
}

/// Collect deterministic raw evidence from every candidate fact represented by one typed refusal.
fn raw_refusal_observations<'a>(facts: impl IntoIterator<Item = &'a HarvestFact>) -> HarvestRefusalObservations {
    let mut observations = HarvestRefusalObservations::default();
    for fact in facts {
        observations.environment.extend(fact.environment_inputs.iter().cloned());
        observations.link.extend(fact.link_observations.iter().cloned());
        observations.tool.extend(fact.tool_observations.iter().cloned());
    }
    observations.environment.sort();
    observations.environment.dedup();
    observations.link.sort();
    observations.link.dedup();
    observations.tool.sort();
    observations.tool.dedup();
    observations
}

/// Observe one unit, or say why it cannot be proposed.
fn observe_unit(
    capture: &OvenLegacyCargoSelectedUnitCapture,
    compiler: &OvenLegacyCargoSelectedCompilerContext,
    unit: &OvenLegacyCargoSelectedUnit,
    profile: &str,
    rustc_executable_identity: &str,
) -> Result<Observation, HarvestRefusal> {
    let refuse = |reason: HarvestRefusalReason, detail: String| HarvestRefusal {
        package: unit.package.clone(),
        version: unit.package_version.clone(),
        reason,
        detail,
        observations: HarvestRefusalObservations::default(),
    };
    if is_build_script_unit(unit) {
        // Named by what it is, never by its position: Cargo orders the unit graph differently between runs, and a
        // refusal list must be the same bytes for the same closure.
        return Err(refuse(
            HarvestRefusalReason::BuildScriptUnit,
            "run-custom-build execution node; the package's library unit is harvested on its own".to_string(),
        ));
    }
    let Some(registry_source) = unit.registry_source.as_ref() else {
        return Err(refuse(
            HarvestRefusalReason::NotRegistryBacked,
            unit.package_source
                .clone()
                .unwrap_or_else(|| "no package source".to_string()),
        ));
    };
    // Cargo's lock spells the checksum bare; the registry binds it as a `sha256:` identity.
    let checksum = canonical_checksum(&registry_source.checksum).ok_or_else(|| {
        refuse(
            HarvestRefusalReason::MalformedChecksum,
            "registry checksum is not a sha256 identity".to_string(),
        )
    })?;
    if let Some(platform) = unit.platform.as_deref()
        && platform != compiler.target
    {
        return Err(refuse(
            HarvestRefusalReason::TargetMismatch,
            format!("compiled for `{platform}`, capture target is `{}`", compiler.target),
        ));
    }

    // ---- The unit's build-script edge, if any ----
    let mut script_edges = unit.dependencies.iter().filter_map(|dependency| {
        capture
            .units
            .get(dependency.unit_index)
            .filter(|candidate| is_build_script_unit(candidate))
            .map(|build_unit| (dependency, build_unit))
    });
    let edge = script_edges.next();
    if script_edges.next().is_some() {
        return Err(refuse(
            HarvestRefusalReason::MultipleBuildScriptEdges,
            "more than one run-custom-build edge".to_string(),
        ));
    }
    let facts: Option<&OvenLegacyCargoBuildScriptFacts> = match edge {
        Some((dependency, build_unit)) => {
            let facts = dependency
                .build_script
                .as_ref()
                .or(build_unit.build_script.as_ref())
                .ok_or_else(|| {
                    refuse(
                        HarvestRefusalReason::OutputNotRetained,
                        "build-script edge has no retained facts".to_string(),
                    )
                })?;
            if facts.output.is_none() {
                return Err(refuse(
                    HarvestRefusalReason::OutputNotRetained,
                    "OUT_DIR was not inventoried".to_string(),
                ));
            }
            Some(facts)
        }
        None => None,
    };

    // ---- The fact, in the reader's shape ----
    let mut features = unit.effective_features.clone();
    features.sort();
    features.dedup();
    let mut cfg = facts.map(|facts| facts.cfgs.clone()).unwrap_or_default();
    cfg.sort();
    cfg.dedup();
    let output = facts.and_then(|facts| facts.output.as_ref());
    if output.is_some_and(|output| !is_sha256_identity(&output.digest)) {
        return Err(refuse(
            HarvestRefusalReason::MalformedOutput,
            "retained output tree has no sha256 digest".to_string(),
        ));
    }
    // The compiler probes this unit's build script ran, and the files that carried their answers. A probe's own
    // output is not a generated input: nothing the package compiles reads it, so it is neither `out` nor a product.
    // It is recognized only by the path and bytes the capture recorded when the probe finished; a file at that path
    // holding other bytes was rewritten by the script, and the harvest refuses rather than guess which it is.
    let unit_probes = match (facts, edge) {
        (Some(facts), Some((_, build_unit))) => capture
            .build_script_tool_probes
            .iter()
            .filter(|probe| probe.package_id == build_unit.package_id && probe.out_dir == facts.out_dir)
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    // Several probes may write one path in turn (rustix probes every feature into `rustix_test_can_compile`), so a
    // path maps to every set of bytes a probe left there.
    let mut probe_outputs = BTreeMap::<&str, BTreeSet<&str>>::new();
    for probe_output in unit_probes.iter().filter_map(|probe| probe.output.as_ref()) {
        probe_outputs
            .entry(probe_output.relative_path.as_str())
            .or_default()
            .insert(probe_output.digest.as_str());
    }
    let members = output.map(|output| output.members.as_slice()).unwrap_or_default();
    if let Some(member) = members.iter().find(|member| {
        probe_outputs
            .get(member.path.as_str())
            .is_some_and(|digests| !digests.contains(member.digest.as_str()))
    }) {
        return Err(refuse(
            HarvestRefusalReason::ConflictingObservations,
            format!(
                "OUT_DIR member `{}` is a probe's output rewritten by the script",
                member.path
            ),
        ));
    }
    let generated_members = members
        .iter()
        .filter(|member| !probe_outputs.contains_key(member.path.as_str()));
    let mut out = Vec::new();
    let mut names = BTreeSet::new();
    for member in generated_members.clone() {
        if safe_relative(&member.path).is_none() || !is_sha256_identity(&member.digest) {
            return Err(refuse(
                HarvestRefusalReason::MalformedOutput,
                format!(
                    "retained member `{}` is not a plain relative path with a sha256 digest",
                    member.path
                ),
            ));
        }
        if !names.insert(member.path.as_str()) {
            return Err(refuse(
                HarvestRefusalReason::MalformedOutput,
                format!("retained member `{}` is inventoried twice", member.path),
            ));
        }
        out.push(RustFactOut {
            name: member.path.clone(),
            path: format!("{HARVEST_OUT_DIRECTORY}/{}", member.path),
            digest: member.digest.clone(),
        });
    }
    out.sort_by(|left, right| left.name.cmp(&right.name));
    let mut products = generated_members
        .map(|member| HarvestObservedProduct {
            owner_relative_path: member.path.clone(),
            digest: member.digest.clone(),
        })
        .collect::<Vec<_>>();
    products.sort();

    // ---- Environment: prove binding-derived constants or retain an exact owner-relative input ----
    let mut environment_inputs = Vec::new();
    if let Some(facts) = facts {
        for (name, value) in &facts.environment {
            if let Some(expected) = binding_derived_environment_value(name, &features, profile, &compiler.target_cfg) {
                if value != &expected {
                    return Err(refuse(
                        HarvestRefusalReason::BindingDerivedEnvironmentMismatch,
                        name.clone(),
                    ));
                }
                continue;
            }
            let Some(owner_relative_path) = owner_relative_path(Path::new(value), &facts.out_dir) else {
                return Err(refuse(HarvestRefusalReason::EnvironmentObserved, name.clone()));
            };
            let Some(product) = products
                .iter()
                .find(|product| product.owner_relative_path == owner_relative_path)
            else {
                return Err(refuse(HarvestRefusalReason::EnvironmentObserved, name.clone()));
            };
            environment_inputs.push(HarvestEnvironmentInput {
                name: name.clone(),
                owner_relative_path,
                digest: product.digest.clone(),
            });
        }
    }
    environment_inputs.sort();

    // ---- Native-link paths retain ownership, never their publisher staging coordinates ----
    let mut link_observations = Vec::new();
    if let Some(facts) = facts
        && (!facts.linked_libraries.is_empty() || !facts.linked_paths.is_empty())
    {
        let mut libraries = facts.linked_libraries.clone();
        libraries.sort();
        libraries.dedup();
        let mut search_paths = Vec::new();
        for value in &facts.linked_paths {
            let (kind, path) = value.split_once('=').unwrap_or(("all", value));
            let Some(owner_relative_path) = owner_relative_path(Path::new(path), &facts.out_dir) else {
                return Err(refuse(
                    HarvestRefusalReason::LinkedPaths,
                    "link search path is not owned by the retained output".to_string(),
                ));
            };
            search_paths.push(HarvestLinkSearchPath {
                kind: kind.to_string(),
                owner_relative_path,
            });
        }
        search_paths.sort();
        search_paths.dedup();
        let output_tree_digest = output.map(|output| output.digest.clone()).ok_or_else(|| {
            refuse(
                HarvestRefusalReason::OutputNotRetained,
                "link products were not retained".to_string(),
            )
        })?;
        link_observations.push(HarvestLinkObservation {
            libraries,
            search_paths,
            output_tree_digest,
            products: products.clone(),
            name: None,
            executable: None,
            objects: Vec::new(),
            environment: Vec::new(),
            sources: Vec::new(),
            library: None,
        });
    }
    let publisher_links = facts
        .into_iter()
        .flat_map(|facts| facts.publisher_work.iter())
        .filter(|work| matches!(work.role, oven_model::manifest::RustFactProducerRole::Link))
        .collect::<Vec<_>>();
    if let ([observation], [work]) = (link_observations.as_mut_slice(), publisher_links.as_slice()) {
        observation.name = Some(work.name.clone());
        observation.executable = work.executable.clone();
        observation.objects = work.objects.clone();
        observation.environment = work.environment.clone();
        observation.sources = work.inputs.clone();
        observation.library = work.library.clone();
    }

    // ---- Tool probes retain their target domains, invocation identity and generated product identities ----
    let mut tool_observations = Vec::new();
    let output_tree_digest = output.map(|output| output.digest.clone());
    for probe in &unit_probes {
        let Some(output_tree_digest) = output_tree_digest.clone() else {
            return Err(refuse(
                HarvestRefusalReason::OutputNotRetained,
                "tool products were not retained".to_string(),
            ));
        };
        tool_observations.push(HarvestToolObservation {
            target_context: probe.target_context.clone(),
            rustc_target: probe.rustc_target.clone(),
            probe_digest: probe.digest.clone(),
            executable_identity: rustc_executable_identity.to_string(),
            output_tree_digest,
            products: products.clone(),
            name: None,
            executable: None,
            arguments: Vec::new(),
            environment: Vec::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
        });
    }

    // ---- A compiler probe answers only through `cfg` ----
    // RFC 119 records a probe's answer, not the probe. A compiler probe is a bounded, non-linking rustc invocation
    // that emits only metadata or IR, so its answer is a function of the compiler and the crate source, which the
    // record already binds by `toolchain`, `target` and `source.checksum`, and it produces no input the package
    // compiles. The probe digests therefore stay evidence, and two units whose probes were spelled differently (a host
    // unit's opt-level) but answered the same fold into one fact. Whatever else the script left in OUT_DIR is its
    // ordinary `out`, as for a script that probes nothing. A probe of another target is refused until a record binds
    // that target.
    let mut compiler_probes = Vec::new();
    if tool_observations
        .iter()
        .all(|observation| observation.rustc_target == compiler.target)
    {
        compiler_probes = tool_observations
            .drain(..)
            .map(|observation| observation.probe_digest)
            .collect();
    }
    if let (Some(facts), Some(output_tree_digest)) = (facts, output.map(|output| output.digest.clone())) {
        for work in facts
            .publisher_work
            .iter()
            .filter(|work| matches!(work.role, oven_model::manifest::RustFactProducerRole::Tool))
        {
            let encoded = serde_json::to_vec(work).map_err(|error| {
                refuse(
                    HarvestRefusalReason::ToolProbes,
                    format!("tool observation could not be encoded: {error}"),
                )
            })?;
            tool_observations.push(HarvestToolObservation {
                target_context: compiler.host.clone(),
                rustc_target: work.target.clone(),
                probe_digest: digest_bytes(&encoded),
                executable_identity: work
                    .executable
                    .as_ref()
                    .map(|executable| executable.digest.clone())
                    .unwrap_or_default(),
                output_tree_digest: output_tree_digest.clone(),
                products: products.clone(),
                name: Some(work.name.clone()),
                executable: work.executable.clone(),
                arguments: work.arguments.clone(),
                environment: work.environment.clone(),
                inputs: work.inputs.clone(),
                outputs: work.outputs.clone(),
            });
        }
    }
    tool_observations.sort();
    tool_observations.dedup();
    Ok(Observation {
        fact: HarvestFact {
            toolchain: compiler.toolchain.clone(),
            target: compiler.target.clone(),
            profile: profile.to_string(),
            features,
            cfg,
            out,
            environment_inputs,
            link_observations,
            tool_observations,
            link: Vec::new(),
            tool: Vec::new(),
        },
        source: HarvestSource {
            registry: registry_index_of(&registry_source.registry),
            checksum,
        },
        out_relative_root: output.map(|output| output.relative_root.clone()),
        compiler_probes,
    })
}

/// Derive the recognized script-emitted constants whose bytes are already fixed by the selected binding.
fn binding_derived_environment_value(
    name: &str,
    features: &[String],
    profile: &str,
    target_cfg: &oven_rustc::rustc::OvenSelectedRustFacetCfgSnapshot,
) -> Option<String> {
    match name {
        "CFG_CARGO_FEATURES" => Some(format!("{features:?}")),
        "CFG_OPT_LEVEL" => Some(if profile == "release" { "3" } else { "0" }.to_string()),
        "CFG_TARGET_FEATURES" => {
            let mut target_features = target_cfg
                .values
                .get("target_feature")
                .into_iter()
                .flatten()
                .flat_map(|value| value.split(','))
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            target_features.sort();
            target_features.dedup();
            Some(format!("{target_features:?}"))
        }
        _ => None,
    }
}

/// Rebind one path below `owner` to a portable owner-relative coordinate.
fn owner_relative_path(path: &Path, owner: &Path) -> Option<String> {
    let relative = path.strip_prefix(owner).ok()?;
    if relative.as_os_str().is_empty() {
        return Some(".".to_string());
    }
    let value = relative.to_str()?;
    safe_relative(value).map(|safe| safe.to_string_lossy().into_owned())
}

/// The execution node that supplies build-script facts to a consumer edge; the same test the projection applies.
fn is_build_script_unit(unit: &OvenLegacyCargoSelectedUnit) -> bool {
    unit.mode == "run-custom-build" && unit.target_kinds.iter().any(|kind| kind == "custom-build")
}

/// The registry index a Cargo source id names: `registry+https://…` becomes `https://…`.
///
/// The registry records `source.registry` as the index URL; Cargo's source-kind prefix is transport spelling.
fn registry_index_of(source: &str) -> String {
    source.strip_prefix("registry+").unwrap_or(source).to_string()
}

// ============================================================================
// Writing a report
// ============================================================================

/// Canonical bytes of one proposal: sorted keys, two-space indentation, trailing newline.
///
/// `serde_json`'s object map is ordered, so a round trip through `Value` sorts the keys; the result is the same
/// bytes for the same facts on every machine, which is what makes a rewrite comparable to what is on disk.
pub fn canonical_proposal_bytes(proposal: &HarvestProposal) -> Result<Vec<u8>, OvenLegacyCargoError> {
    canonical_json_bytes(proposal)
}

/// Canonical bytes of the refusal list.
pub fn canonical_refusals_bytes(refusals: &[HarvestRefusal]) -> Result<Vec<u8>, OvenLegacyCargoError> {
    canonical_json_bytes(refusals)
}

/// Sorted-key pretty JSON with a trailing newline.
fn canonical_json_bytes<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, OvenLegacyCargoError> {
    let value = serde_json::to_value(value)
        .map_err(|error| OvenLegacyCargoError::Plan(format!("could not encode harvest report: {error}")))?;
    let mut bytes = serde_json::to_vec_pretty(&value)
        .map_err(|error| OvenLegacyCargoError::Plan(format!("could not encode harvest report: {error}")))?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// The directory name each proposal is written under, relative to the report root.
///
/// `<name>-<version>-<profile>`, as the registry's proposal contract spells it, when the report holds one selection
/// for that package version and profile. A capture can hold more than one (Cargo's resolver keeps host and target
/// feature sets apart), and then every directory for that package version and profile also carries a short digest
/// of its selection so the names stay stable whichever other selections appear.
pub fn proposal_directory_names(report: &HarvestReport) -> Result<Vec<String>, OvenLegacyCargoError> {
    let fact_of = |proposal: &HarvestProposal| {
        proposal.rust.facts.first().cloned().ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "harvest proposal for `{}-{}` carries no fact",
                proposal.project.name, proposal.project.version
            ))
        })
    };
    let mut per_version = BTreeMap::<(String, String, String), usize>::new();
    for proposal in &report.proposals {
        let fact = fact_of(proposal)?;
        *per_version
            .entry((
                proposal.project.name.clone(),
                proposal.project.version.clone(),
                fact.profile.clone(),
            ))
            .or_default() += 1;
    }
    report
        .proposals
        .iter()
        .map(|proposal| {
            let fact = fact_of(proposal)?;
            let base = format!(
                "{}-{}-{}",
                proposal.project.name, proposal.project.version, fact.profile
            );
            let count = per_version
                .get(&(
                    proposal.project.name.clone(),
                    proposal.project.version.clone(),
                    fact.profile.clone(),
                ))
                .copied()
                .unwrap_or(1);
            if count == 1 {
                return Ok(base);
            }
            let selection = serde_json::to_vec(&(&fact.toolchain, &fact.target, &fact.profile, &fact.features))
                .map_err(|error| OvenLegacyCargoError::Plan(format!("could not encode selection: {error}")))?;
            let digest = digest_bytes(&selection);
            let short = digest
                .strip_prefix("sha256:")
                .unwrap_or(&digest)
                .chars()
                .take(12)
                .collect::<String>();
            Ok(format!("{base}-{short}"))
        })
        .collect()
}

/// Write the report under `dir`: one `<name>-<version>-<profile>/proposal.json` per proposal with its `out/`
/// members copied beside it, and `refusals-<profile>.json` at the root. Returns every file written, in order.
///
/// `out_dir_sources` is the root the captured `output.relative_root` paths resolve under: the publisher staging
/// during a bake, or the published artifact's materialized root once the staging is gone. Each member's bytes are
/// verified against the digest the proposal names before they are written. The write is idempotent: a file that
/// already holds the same bytes is left alone, and a file that holds different bytes is a refusal, because a harvest
/// that changes its answer for the same directory is a changed freeze, which RFC 119 says needs a new explicit
/// harvest, not a silent overwrite.
pub fn write_harvest_report(
    report: &HarvestReport,
    dir: &Path,
    out_dir_sources: &Path,
) -> Result<Vec<PathBuf>, OvenLegacyCargoError> {
    fs::create_dir_all(dir).map_err(|source| OvenLegacyCargoError::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    let names = proposal_directory_names(report)?;
    let mut written = Vec::new();
    for (proposal, name) in report.proposals.iter().zip(names) {
        let proposal_dir = dir.join(&name);
        fs::create_dir_all(&proposal_dir).map_err(|source| OvenLegacyCargoError::Io {
            path: proposal_dir.clone(),
            source,
        })?;
        // The binding is checked before any member is copied, so a refused rewrite leaves the directory exactly as
        // the earlier harvest wrote it rather than with a stray member no proposal names.
        let proposal_path = proposal_dir.join(HARVEST_PROPOSAL_FILE);
        let already_bound = proposal_already_bound(&proposal_path, proposal, &name)?;
        // ---- Generated inputs, verified against the digests the proposal names ----
        for fact in &proposal.rust.facts {
            for out in &fact.out {
                let relative_root = proposal.out_relative_root.as_deref().ok_or_else(|| {
                    OvenLegacyCargoError::Plan(format!(
                        "harvest proposal for `{name}` names generated input `{}` without a retained root",
                        out.name
                    ))
                })?;
                let member = safe_relative(&out.name).ok_or_else(|| {
                    OvenLegacyCargoError::Plan(format!(
                        "harvest proposal for `{name}` names an unsafe generated input `{}`",
                        out.name
                    ))
                })?;
                let destination_relative = safe_relative(&out.path).ok_or_else(|| {
                    OvenLegacyCargoError::Plan(format!(
                        "harvest proposal for `{name}` names an unsafe committed path `{}`",
                        out.path
                    ))
                })?;
                let source = out_dir_sources.join(relative_root).join(&member);
                let bytes = regular_file_bytes(&source)?;
                if digest_bytes(&bytes) != out.digest {
                    return Err(OvenLegacyCargoError::Plan(format!(
                        "retained generated input `{}` for `{name}` does not match its captured digest",
                        out.name
                    )));
                }
                let destination = proposal_dir.join(&destination_relative);
                write_idempotently(&destination, &bytes, &name)?;
                written.push(destination);
            }
        }
        if !already_bound {
            write_new_file(&proposal_path, &canonical_proposal_bytes(proposal)?)?;
        }
        written.push(proposal_path);
    }
    let refusals_name = harvest_refusals_file_name(&report.profile);
    let refusals_path = dir.join(&refusals_name);
    write_idempotently(
        &refusals_path,
        &canonical_refusals_bytes(&report.refusals)?,
        &refusals_name,
    )?;
    written.push(refusals_path);
    Ok(written)
}

/// Produce and idempotently write one canonical harvest report.
///
/// Standalone and release harvesting share this operation so classification, canonical proposal bytes, retained-byte
/// validation, and refusal writing cannot drift between entry points.
pub fn harvest_registry_units_to_dir(
    capture: &OvenLegacyCargoSelectedUnitCapture,
    evidence: &HarvestEvidenceInputs,
    profile: &str,
    dir: &Path,
    out_dir_sources: &Path,
) -> Result<HarvestReport, OvenLegacyCargoError> {
    let report = harvest_registry_units(capture, evidence, profile)?;
    write_harvest_report(&report, dir, out_dir_sources)?;
    Ok(report)
}

/// A plain relative path with only normal components, or `None`.
fn safe_relative(value: &str) -> Option<PathBuf> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return None;
    }
    Some(path.to_path_buf())
}

/// Whether a proposal binding the same facts is already at `path`; a proposal binding different facts refuses.
///
/// Idempotence is over the binding — project, source and the fact — not over provenance: the note names the
/// checkout the harvest ran at and the evidence names its receipt and publisher, and those move with every commit
/// without the freeze changing. A proposal already present with the same binding is left as it is (its
/// provenance is the earlier, equally valid observation); a different binding is a changed freeze and refuses.
fn proposal_already_bound(
    path: &Path,
    proposal: &HarvestProposal,
    subject: &str,
) -> Result<bool, OvenLegacyCargoError> {
    match fs::read(path) {
        Ok(existing) => {
            let previous: HarvestProposal = serde_json::from_slice(&existing).map_err(|error| {
                OvenLegacyCargoError::Plan(format!(
                    "harvest output {} for `{subject}` already exists and is not a proposal: {error}",
                    path.display()
                ))
            })?;
            if previous.project == proposal.project
                && previous.source == proposal.source
                && previous.rust == proposal.rust
            {
                return Ok(true);
            }
            Err(OvenLegacyCargoError::Plan(format!(
                "harvest output {} for `{subject}` already binds different facts; a changed freeze needs a new output directory",
                path.display()
            )))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(OvenLegacyCargoError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Write `bytes` at `path` unless identical bytes are already there; different bytes are a refusal.
fn write_idempotently(path: &Path, bytes: &[u8], subject: &str) -> Result<(), OvenLegacyCargoError> {
    match fs::read(path) {
        Ok(existing) if existing == bytes => return Ok(()),
        Ok(_) => {
            return Err(OvenLegacyCargoError::Plan(format!(
                "harvest output {} for `{subject}` already exists with different bytes; a changed freeze needs a new output directory",
                path.display()
            )));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(OvenLegacyCargoError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    }
    write_new_file(path, bytes)
}

/// Create `path`'s parent and write `bytes` there.
fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), OvenLegacyCargoError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| OvenLegacyCargoError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    fs::write(path, bytes).map_err(|source| OvenLegacyCargoError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use oven_rustc::rustc::{OvenSelectedRustFacetCfgSnapshot, selected_graph_sha256};
    use tempfile::tempdir;

    use super::super::{
        OvenLegacyCargoBuildScriptToolProbe, OvenLegacyCargoInspectionSourceMember, OvenLegacyCargoSelectedDependency,
        OvenLegacyCargoSelectedGeneratedOutput, OvenLegacyCargoSelectedRegistrySource,
    };
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    const CHECKSUM: &str = "sha256:41d385c7d4ca58e59fc732af25c3983b67ac852c1a25000afe1175de458b67ad";

    fn evidence() -> HarvestEvidenceInputs {
        HarvestEvidenceInputs {
            receipt: Some(format!("sha256:{}", "1".repeat(64))),
            rustc_identity: format!("sha256:{}", "4".repeat(64)),
            ambient_hazards: Vec::new(),
            notes: None,
            cargo_version: "cargo 1.98.0 (fixture)".to_string(),
            cargo_lock_digest: format!("sha256:{}", "2".repeat(64)),
            cargo_manifest_digest: format!("sha256:{}", "3".repeat(64)),
        }
    }

    fn cfg_snapshot() -> OvenSelectedRustFacetCfgSnapshot {
        OvenSelectedRustFacetCfgSnapshot {
            flags: vec!["unix".to_string()],
            values: BTreeMap::new(),
        }
    }

    /// A registry library unit with no edges; the release-shaped fixtures in `selected_graph_projection` add the
    /// transport root and the build-script edge on top of this same shape.
    fn library(package: &str, version: &str, features: &[&str]) -> OvenLegacyCargoSelectedUnit {
        OvenLegacyCargoSelectedUnit {
            package_id: format!("registry+https://github.com/rust-lang/crates.io-index#{package}@{version}"),
            package: package.to_string(),
            package_version: version.to_string(),
            package_source: Some("registry+https://github.com/rust-lang/crates.io-index".to_string()),
            target_name: package.to_string(),
            target_kinds: vec!["lib".to_string()],
            crate_types: vec!["lib".to_string()],
            source_path: PathBuf::from(format!("/transient/{package}/src/lib.rs")),
            artifact_paths: Vec::new(),
            root_module: "src/lib.rs".to_string(),
            edition: "2021".to_string(),
            mode: "build".to_string(),
            platform: Some("x86_64-unknown-linux-gnu".to_string()),
            target_is_explicit: Some(true),
            cfg: Vec::new(),
            effective_features: features.iter().map(|feature| feature.to_string()).collect(),
            dependencies: Vec::new(),
            sysroot_externs: Vec::new(),
            build_script: None,
            registry_source: Some(OvenLegacyCargoSelectedRegistrySource {
                registry: "registry+https://github.com/rust-lang/crates.io-index".to_string(),
                checksum: CHECKSUM.to_string(),
                digest: selected_graph_sha256(b"source"),
                root_module: "src/lib.rs".to_string(),
                members: vec![OvenLegacyCargoInspectionSourceMember {
                    path: "src/lib.rs".to_string(),
                    digest: selected_graph_sha256(b"pub fn fixture() {}\n"),
                }],
            }),
        }
    }

    /// The run-custom-build node of `library`.
    fn build_script_of(library: &OvenLegacyCargoSelectedUnit) -> OvenLegacyCargoSelectedUnit {
        let mut unit = library.clone();
        unit.target_name = "build-script-build".to_string();
        unit.target_kinds = vec!["custom-build".to_string()];
        unit.crate_types = vec!["bin".to_string()];
        unit.root_module = "build.rs".to_string();
        unit.mode = "run-custom-build".to_string();
        unit
    }

    fn facts(cfgs: &[&str], output: Option<OvenLegacyCargoSelectedGeneratedOutput>) -> OvenLegacyCargoBuildScriptFacts {
        OvenLegacyCargoBuildScriptFacts {
            cfgs: cfgs.iter().map(|cfg| cfg.to_string()).collect(),
            environment: BTreeMap::new(),
            linked_libraries: Vec::new(),
            linked_paths: Vec::new(),
            out_dir: PathBuf::from("/transient/out"),
            output,
            publisher_work: Vec::new(),
        }
    }

    /// A retained output tree with stable product bytes for raw-observation contract tests.
    fn retained_products() -> OvenLegacyCargoSelectedGeneratedOutput {
        OvenLegacyCargoSelectedGeneratedOutput {
            relative_root: "generated-outputs/products".to_string(),
            digest: selected_graph_sha256(b"product-tree"),
            members: vec![OvenLegacyCargoInspectionSourceMember {
                path: "libfixture.a".to_string(),
                digest: selected_graph_sha256(b"archive"),
            }],
        }
    }

    /// A retained generated tree shaped like one Cranelift ISLE product.
    fn retained_isle_products() -> OvenLegacyCargoSelectedGeneratedOutput {
        OvenLegacyCargoSelectedGeneratedOutput {
            relative_root: "generated-outputs/isle".to_string(),
            digest: selected_graph_sha256(b"isle products"),
            members: vec![OvenLegacyCargoInspectionSourceMember {
                path: "generated/isle_opt.rs".to_string(),
                digest: selected_graph_sha256(b"generated isle source"),
            }],
        }
    }

    #[test]
    fn harvest_contract_preserves_link_product_byte_identities() -> TestResult {
        let mut linked = facts(&[], Some(retained_products()));
        linked.linked_libraries = vec!["static=fixture".to_string()];
        linked.linked_paths = vec!["native=/transient/out".to_string()];
        let report = harvest_registry_units(
            &capture(vec![(library("fixture-sys", "1.0.0", &[]), Some(linked))]),
            &evidence(),
            "release",
        )?;
        assert!(report.proposals.is_empty());
        let refusal = report
            .refusals
            .iter()
            .find(|refusal| refusal.package == "fixture-sys" && refusal.reason == HarvestRefusalReason::LinkedLibraries)
            .ok_or("owned link work must be refused")?;
        let observation = &refusal.observations.link[0];
        assert_eq!(observation.libraries, ["static=fixture"]);
        assert_eq!(observation.search_paths[0].owner_relative_path, ".");
        assert_eq!(observation.output_tree_digest, selected_graph_sha256(b"product-tree"));
        assert_eq!(observation.products[0].digest, selected_graph_sha256(b"archive"));
        Ok(())
    }

    #[test]
    /// A refused probe (here one of another target) keeps its invocation identity and the OUT_DIR products beside it.
    fn harvest_contract_preserves_tool_probe_and_product_identities() -> TestResult {
        let unit = library("isle-meta", "1.0.0", &[]);
        let build = build_script_of(&unit);
        let mut selected = capture(vec![(unit, Some(facts(&[], Some(retained_products()))))]);
        selected
            .build_script_tool_probes
            .push(OvenLegacyCargoBuildScriptToolProbe {
                package_id: build.package_id,
                out_dir: PathBuf::from("/transient/out"),
                target_context: "x86_64-unknown-linux-gnu".to_string(),
                rustc_target: "aarch64-apple-darwin".to_string(),
                digest: selected_graph_sha256(b"probe"),
                output: None,
            });
        let report = harvest_registry_units(&selected, &evidence(), "release")?;
        assert!(report.proposals.is_empty());
        let refusal = report
            .refusals
            .iter()
            .find(|refusal| refusal.package == "isle-meta" && refusal.reason == HarvestRefusalReason::ToolProbes)
            .ok_or("owned tool work must be refused")?;
        let observation = &refusal.observations.tool[0];
        assert_eq!(observation.probe_digest, selected_graph_sha256(b"probe"));
        assert_eq!(observation.products[0].digest, selected_graph_sha256(b"archive"));
        Ok(())
    }

    #[test]
    /// A build script's compiler probes propose their answers as `cfg` with the probe digests as evidence. A probe's
    /// own output left in OUT_DIR is neither `out` nor a product, while a file the script wrote stays `out`. Units
    /// that asked differently but answered the same fold into one fact; different answers still conflict, and a
    /// probe of another target stays tool work.
    fn harvest_contract_proposes_probe_only_answers_as_cfg() -> TestResult {
        let probed_out = || OvenLegacyCargoSelectedGeneratedOutput {
            relative_root: "generated-outputs/probed".to_string(),
            digest: selected_graph_sha256(b"probed tree"),
            members: vec![
                OvenLegacyCargoInspectionSourceMember {
                    path: "config.rs".to_string(),
                    digest: selected_graph_sha256(b"pub const PRECISION: u64 = 100;"),
                },
                OvenLegacyCargoInspectionSourceMember {
                    path: "rustix_test_can_compile".to_string(),
                    digest: selected_graph_sha256(b"probe metadata"),
                },
            ],
        };
        let unit = library("rustix", "1.1.5", &["std"]);
        let probe = |out_dir: &str, target: &str, spelling: &[u8]| OvenLegacyCargoBuildScriptToolProbe {
            package_id: unit.package_id.clone(),
            out_dir: PathBuf::from(out_dir),
            target_context: "x86_64-unknown-linux-gnu".to_string(),
            rustc_target: target.to_string(),
            digest: selected_graph_sha256(spelling),
            output: Some(crate::rustc_trace::OvenLegacyStdinProbeOutput {
                relative_path: "rustix_test_can_compile".to_string(),
                digest: selected_graph_sha256(b"probe metadata"),
            }),
        };
        let selection = |host_answers: &[&str], host_probe_target: &str| {
            let mut target_facts = facts(&["static_assertions"], Some(probed_out()));
            target_facts.out_dir = PathBuf::from("/transient/target-out");
            let mut host_facts = facts(host_answers, Some(probed_out()));
            host_facts.out_dir = PathBuf::from("/transient/host-out");
            let mut selected = capture(vec![
                (unit.clone(), Some(target_facts)),
                (unit.clone(), Some(host_facts)),
            ]);
            // An earlier probe of the target unit wrote other bytes to the same path before the last one replaced
            // them, as rustix's feature probes do; the member must match one probe's bytes, not every probe's.
            let mut earlier = probe(
                "/transient/target-out",
                "x86_64-unknown-linux-gnu",
                b"earlier probe at opt-level 3",
            );
            if let Some(output) = earlier.output.as_mut() {
                output.digest = selected_graph_sha256(b"earlier probe metadata");
            }
            selected.build_script_tool_probes.extend([
                earlier,
                probe(
                    "/transient/target-out",
                    "x86_64-unknown-linux-gnu",
                    b"probe at opt-level 3",
                ),
                probe("/transient/host-out", host_probe_target, b"probe at opt-level 0"),
            ]);
            selected
        };

        let report = harvest_registry_units(
            &selection(&["static_assertions"], "x86_64-unknown-linux-gnu"),
            &evidence(),
            "release",
        )?;
        assert!(report.refusals.iter().all(|refusal| refusal.package != "rustix"));
        let proposal = report
            .proposals
            .iter()
            .find(|proposal| proposal.project.name == "rustix")
            .ok_or("probe-only answers must be proposed")?;
        assert_eq!(proposal.rust.facts[0].cfg, ["static_assertions"]);
        assert!(proposal.rust.facts[0].tool.is_empty());
        assert_eq!(
            proposal.rust.facts[0]
                .out
                .iter()
                .map(|member| member.name.as_str())
                .collect::<Vec<_>>(),
            ["config.rs"],
            "the probe's own output is not a generated input; the script's file is"
        );
        let mut probes = vec![
            selected_graph_sha256(b"earlier probe at opt-level 3"),
            selected_graph_sha256(b"probe at opt-level 3"),
            selected_graph_sha256(b"probe at opt-level 0"),
        ];
        probes.sort();
        assert_eq!(proposal.evidence.compiler_probes, probes);
        let written = serde_json::to_value(proposal)?;
        assert_eq!(written["evidence"]["compiler_probes"].as_array().map(Vec::len), Some(3));
        assert!(written["rust"]["facts"][0].get("tool").is_none());

        let disagreeing = harvest_registry_units(
            &selection(&["other_answer"], "x86_64-unknown-linux-gnu"),
            &evidence(),
            "release",
        )?;
        assert!(
            disagreeing.refusals.iter().any(|refusal| refusal.package == "rustix"
                && refusal.reason == HarvestRefusalReason::ConflictingObservations)
        );

        let foreign = harvest_registry_units(
            &selection(&["static_assertions"], "aarch64-apple-darwin"),
            &evidence(),
            "release",
        )?;
        assert!(
            foreign
                .proposals
                .iter()
                .all(|proposal| proposal.project.name != "rustix")
        );
        assert!(foreign.refusals.iter().any(|refusal| refusal.package == "rustix"));

        // A file at a probe's output path holding other bytes was rewritten by the script: refuse, never choose.
        let mut rewritten = selection(&["static_assertions"], "x86_64-unknown-linux-gnu");
        for probe in &mut rewritten.build_script_tool_probes {
            if let Some(output) = probe.output.as_mut() {
                output.digest = selected_graph_sha256(b"what the probe wrote before the script replaced it");
            }
        }
        let rewritten = harvest_registry_units(&rewritten, &evidence(), "release")?;
        assert!(
            rewritten
                .proposals
                .iter()
                .all(|proposal| proposal.project.name != "rustix")
        );
        assert!(rewritten.refusals.iter().any(|refusal| refusal.package == "rustix"
            && refusal.reason == HarvestRefusalReason::ConflictingObservations
            && refusal.detail.contains("rewritten by the script")));
        Ok(())
    }

    #[test]
    fn harvest_contract_converts_only_rebindable_environment_inputs() -> TestResult {
        let mut observed = facts(&[], Some(retained_products()));
        observed
            .environment
            .insert("FIXTURE_ARCHIVE".to_string(), "/transient/out/libfixture.a".to_string());
        let report = harvest_registry_units(
            &capture(vec![(library("fixture", "1.0.0", &[]), Some(observed))]),
            &evidence(),
            "release",
        )?;
        assert!(report.proposals.is_empty());
        let refusal = report
            .refusals
            .iter()
            .find(|refusal| refusal.package == "fixture" && refusal.reason == HarvestRefusalReason::EnvironmentObserved)
            .ok_or("retained environment input must be refused until its typed record exists")?;
        let input = &refusal.observations.environment[0];
        assert_eq!(input.name, "FIXTURE_ARCHIVE");
        assert_eq!(input.owner_relative_path, "libfixture.a");
        assert_eq!(input.digest, selected_graph_sha256(b"archive"));
        Ok(())
    }

    #[test]
    fn harvest_contract_refuses_unmodelled_environment_values() -> TestResult {
        let mut observed = facts(&[], Some(retained_products()));
        observed
            .environment
            .insert("SECRET".to_string(), "ambient-value".to_string());
        let report = harvest_registry_units(
            &capture(vec![(library("fixture", "1.0.0", &[]), Some(observed))]),
            &evidence(),
            "release",
        )?;
        assert!(report.proposals.is_empty());
        assert!(
            report
                .refusals
                .iter()
                .any(|refusal| refusal.reason == HarvestRefusalReason::EnvironmentObserved)
        );
        Ok(())
    }

    #[test]
    fn harvest_contract_proves_binding_derived_environment_constants() -> TestResult {
        let mut observed = facts(&[], Some(retained_products()));
        observed.environment = BTreeMap::from([
            ("CFG_CARGO_FEATURES".to_string(), "[\"arch\", \"default\"]".to_string()),
            ("CFG_OPT_LEVEL".to_string(), "3".to_string()),
            ("CFG_TARGET_FEATURES".to_string(), "[\"neon\", \"sha2\"]".to_string()),
        ]);
        let mut selected = capture(vec![(library("libm", "0.2.16", &["default", "arch"]), Some(observed))]);
        let compiler = selected.compiler.as_mut().ok_or("fixture compiler context missing")?;
        compiler
            .target_cfg
            .values
            .insert("target_feature".to_string(), vec!["sha2,neon".to_string()]);

        let report = harvest_registry_units(&selected, &evidence(), "release")?;
        let proposal = report
            .proposals
            .iter()
            .find(|proposal| proposal.project.name == "libm")
            .ok_or("libm binding-derived constants must harvest")?;
        assert!(proposal.rust.facts[0].environment_inputs.is_empty());
        assert!(proposal.admitted_record().is_ok());
        assert!(
            !serde_json::to_string(proposal)?.contains("CFG_"),
            "proved constants need no fact key"
        );
        Ok(())
    }

    #[test]
    fn harvest_contract_refuses_a_binding_derived_constant_that_disagrees() -> TestResult {
        for (name, value) in [
            ("CFG_CARGO_FEATURES", "[\"wrong\"]"),
            ("CFG_OPT_LEVEL", "0"),
            ("CFG_TARGET_FEATURES", "[\"wrong\"]"),
        ] {
            let mut observed = facts(&[], Some(retained_products()));
            observed.environment.insert(name.to_string(), value.to_string());
            let report = harvest_registry_units(
                &capture(vec![(library("libm", "0.2.16", &["arch", "default"]), Some(observed))]),
                &evidence(),
                "release",
            )?;
            assert!(report.proposals.is_empty());
            assert!(report.refusals.iter().any(|refusal| {
                refusal.reason == HarvestRefusalReason::BindingDerivedEnvironmentMismatch && refusal.detail == name
            }));
        }
        Ok(())
    }

    #[test]
    fn harvest_contract_emits_explicit_empty_facts() -> TestResult {
        let report = harvest_registry_units(
            &capture(vec![(library("empty", "1.0.0", &[]), None)]),
            &evidence(),
            "release",
        )?;
        assert_eq!(report.proposals.len(), 1);
        assert_eq!(
            report.proposals[0].rust.facts[0].effect_classes(),
            [HarvestEffectClass::Empty]
        );
        Ok(())
    }

    #[test]
    fn admitted_proposal_converts_cfg_out_and_refuses_raw_work() -> TestResult {
        let clean = harvest_registry_units(
            &capture(vec![(library("empty", "1.0.0", &[]), None)]),
            &evidence(),
            "release",
        )?;
        let admitted = clean.proposals[0].admitted_record()?;
        assert!(admitted.cfg.is_empty() && admitted.out.is_empty());
        assert_eq!(admitted.harvested_from, evidence().receipt);

        let mut linked = facts(&[], Some(retained_products()));
        linked.linked_libraries = vec!["static=fixture".to_string()];
        let raw = harvest_registry_units(
            &capture(vec![(library("fixture-sys", "1.0.0", &[]), Some(linked))]),
            &evidence(),
            "release",
        )?;
        let link = raw
            .refusals
            .iter()
            .find(|refusal| refusal.package == "fixture-sys")
            .ok_or("link refusal missing")?
            .observations
            .link
            .clone();
        let mut raw_proposal = clean.proposals[0].clone();
        raw_proposal.rust.facts[0].link_observations = link;
        assert_eq!(
            raw_proposal.admitted_record().err(),
            Some(HarvestAdmissionRefusal::UnresolvedLinkObservations)
        );

        let mut environment = facts(&[], Some(retained_products()));
        environment
            .environment
            .insert("FIXTURE_ARCHIVE".to_string(), "/transient/out/libfixture.a".to_string());
        let raw = harvest_registry_units(
            &capture(vec![(library("fixture", "1.0.0", &[]), Some(environment))]),
            &evidence(),
            "release",
        )?;
        let environment = raw
            .refusals
            .iter()
            .find(|refusal| refusal.package == "fixture")
            .ok_or("environment refusal missing")?
            .observations
            .environment
            .clone();
        let mut raw_proposal = clean.proposals[0].clone();
        raw_proposal.rust.facts[0].environment_inputs = environment;
        assert_eq!(
            raw_proposal.admitted_record().err(),
            Some(HarvestAdmissionRefusal::UnresolvedEnvironmentInputs)
        );
        Ok(())
    }

    /// Complete native work shaped like the bundled C archives emitted by blake3 and zstd-sys.
    fn complete_link_observation(name: &str, source_path: &str) -> HarvestLinkObservation {
        let digest = selected_graph_sha256(name.as_bytes());
        HarvestLinkObservation {
            libraries: vec![format!("static={name}")],
            search_paths: vec![HarvestLinkSearchPath {
                kind: "native".to_string(),
                owner_relative_path: ".".to_string(),
            }],
            output_tree_digest: selected_graph_sha256(format!("{name}-products").as_bytes()),
            products: vec![HarvestObservedProduct {
                owner_relative_path: format!("lib{name}.a"),
                digest: selected_graph_sha256(format!("lib{name}.a").as_bytes()),
            }],
            name: Some(name.to_string()),
            executable: Some(RustFactExecutable {
                name: "clang".to_string(),
                owner: selected_graph_sha256(b"publisher-toolchain"),
                path: "bin/clang".to_string(),
                digest: selected_graph_sha256(b"clang"),
            }),
            objects: vec![RustFactLinkObject {
                name: format!("{name}.o"),
                language: oven_model::manifest::RustFactLinkLanguage::C,
                arguments: vec![
                    RustFactArgument::Input {
                        input: "sources".to_string(),
                    },
                    RustFactArgument::Output {
                        output: format!("{name}.o"),
                    },
                ],
            }],
            environment: Vec::new(),
            sources: vec![RustFactArtifact {
                name: "sources".to_string(),
                kind: RustFactArtifactKind::Tree,
                path: source_path.to_string(),
                digest,
                members: vec![oven_model::manifest::RustFactArtifactMember {
                    path: "fixture.c".to_string(),
                    digest: selected_graph_sha256(b"fixture source"),
                }],
            }],
            library: Some(RustFactLibrary {
                name: name.to_string(),
                kind: RustFactLibraryKind::Static,
            }),
        }
    }

    /// Complete generator work shaped like Cranelift's ISLE source-to-Rust generation.
    fn complete_isle_observation() -> HarvestToolObservation {
        HarvestToolObservation {
            target_context: "x86_64-unknown-linux-gnu".to_string(),
            rustc_target: "x86_64-unknown-linux-gnu".to_string(),
            probe_digest: selected_graph_sha256(b"isle invocation"),
            executable_identity: selected_graph_sha256(b"isle executable"),
            output_tree_digest: selected_graph_sha256(b"isle products"),
            products: vec![HarvestObservedProduct {
                owner_relative_path: "generated/isle_opt.rs".to_string(),
                digest: selected_graph_sha256(b"generated isle source"),
            }],
            name: Some("isle".to_string()),
            executable: Some(RustFactExecutable {
                name: "isle".to_string(),
                owner: selected_graph_sha256(b"cranelift-isle provider"),
                path: "bin/isle".to_string(),
                digest: selected_graph_sha256(b"isle executable"),
            }),
            arguments: vec![
                RustFactArgument::Input {
                    input: "isle-source".to_string(),
                },
                RustFactArgument::Output {
                    output: "generated-rust".to_string(),
                },
            ],
            environment: Vec::new(),
            inputs: vec![RustFactArtifact {
                name: "isle-source".to_string(),
                kind: RustFactArtifactKind::File,
                path: "src/opts.isle".to_string(),
                digest: selected_graph_sha256(b"isle source"),
                members: Vec::new(),
            }],
            outputs: vec![RustFactOutput {
                name: "generated-rust".to_string(),
                kind: RustFactArtifactKind::File,
                path: "generated/isle_opt.rs".to_string(),
            }],
        }
    }

    #[test]
    fn admitted_proposal_converts_blake3_and_zstd_shaped_link_observations() -> TestResult {
        for (name, source_path) in [("blake3", "c"), ("zstd", "zstd/lib")] {
            let complete = complete_link_observation(name, source_path);
            let link = link_record_from_observation(&complete, "x86_64-unknown-linux-gnu")?;
            let mut observed = facts(&[], Some(retained_products()));
            observed.linked_libraries = vec![format!("static={name}")];
            observed.linked_paths = vec!["native=/transient/out".to_string()];
            observed.publisher_work = vec![RustFactWorkObservation::from(link)];
            let report = harvest_registry_units(
                &capture(vec![(library(name, "1.0.0", &[]), Some(observed))]),
                &evidence(),
                "release",
            )?;
            let proposal = report
                .proposals
                .first()
                .ok_or("complete link observation was not proposed")?;
            let admitted = proposal.admitted_record()?;
            assert_eq!(admitted.link.len(), 1);
            assert_eq!(admitted.link[0].library.name, name);
            assert_eq!(admitted.link[0].sources[0].path, source_path);
            assert!(proposal.rust.facts[0].link_observations.is_empty());
            assert_eq!(proposal.rust.facts[0].link.len(), 1);
            assert!(proposal.rust.facts[0].out.is_empty());
            let encoded = serde_json::to_value(proposal)?;
            assert!(encoded["rust"]["facts"][0]["link"][0]["sources"][0]["digest"].is_string());
            assert!(encoded["rust"]["facts"][0].get("link_observations").is_none());
        }
        Ok(())
    }

    #[test]
    fn admitted_proposal_converts_cranelift_isle_shaped_tool_observation() -> TestResult {
        let complete = complete_isle_observation();
        let tool = tool_record_from_observation(&complete, "x86_64-unknown-linux-gnu")?;
        let mut observed = facts(&[], Some(retained_isle_products()));
        observed.publisher_work = vec![RustFactWorkObservation::from(tool)];
        let report = harvest_registry_units(
            &capture(vec![(library("cranelift-codegen", "1.0.0", &[]), Some(observed))]),
            &evidence(),
            "release",
        )?;
        let proposal = report
            .proposals
            .first()
            .ok_or("complete ISLE observation was not proposed")?;
        let admitted = proposal.admitted_record()?;
        assert_eq!(admitted.tool.len(), 1);
        assert_eq!(admitted.tool[0].name, "isle");
        assert_eq!(admitted.tool[0].outputs[0].path, "generated/isle_opt.rs");
        assert!(proposal.rust.facts[0].tool_observations.is_empty());
        assert_eq!(proposal.rust.facts[0].tool.len(), 1);
        assert!(proposal.rust.facts[0].out.is_empty());
        let encoded = serde_json::to_value(proposal)?;
        assert!(encoded["rust"]["facts"][0]["tool"][0]["executable"]["digest"].is_string());
        assert!(
            encoded["rust"]["facts"][0]["tool"][0]["outputs"][0]
                .get("digest")
                .is_none()
        );
        assert!(encoded["rust"]["facts"][0].get("tool_observations").is_none());
        let output = tempdir()?;
        let retained = tempdir()?;
        let written = write_harvest_report(&report, output.path(), retained.path())?;
        assert!(written.iter().all(|path| !path.to_string_lossy().contains("/out/")));
        Ok(())
    }

    #[test]
    fn complete_tool_output_is_not_also_retained_as_out() -> TestResult {
        let complete = complete_isle_observation();
        let tool = tool_record_from_observation(&complete, "x86_64-unknown-linux-gnu")?;
        let mut observed = facts(&[], Some(retained_isle_products()));
        observed.publisher_work = vec![RustFactWorkObservation::from(tool)];
        let report = harvest_registry_units(
            &capture(vec![(library("cranelift-codegen", "1.0.0", &[]), Some(observed))]),
            &evidence(),
            "release",
        )?;
        let fact = &report
            .proposals
            .first()
            .ok_or("complete ISLE observation was not proposed")?
            .rust
            .facts[0];
        assert!(fact.out.is_empty());
        assert_eq!(fact.tool[0].outputs[0].path, "generated/isle_opt.rs");
        Ok(())
    }

    #[test]
    fn disagreeing_tool_output_and_out_bytes_are_conflicting_observations() -> TestResult {
        let clean = harvest_registry_units(
            &capture(vec![(library("cranelift-codegen", "1.0.0", &[]), None)]),
            &evidence(),
            "release",
        )?;
        let mut proposal = clean.proposals[0].clone();
        proposal.rust.facts[0].out = vec![RustFactOut {
            name: "generated/isle_opt.rs".to_string(),
            path: "out/generated/isle_opt.rs".to_string(),
            digest: selected_graph_sha256(b"generated isle source"),
        }];
        let mut observation = complete_isle_observation();
        observation.products[0].digest = selected_graph_sha256(b"different");
        proposal.rust.facts[0].tool_observations = vec![observation];
        let reason = proposal
            .admitted_record()
            .err()
            .ok_or("disagreeing tool/out bytes were admitted")?;
        assert_eq!(reason, HarvestAdmissionRefusal::ConflictingObservations);
        assert_eq!(
            admission_refusal(&proposal, reason).reason,
            HarvestRefusalReason::ConflictingObservations
        );
        Ok(())
    }

    #[test]
    fn incomplete_work_refuses_independently_of_profile() -> TestResult {
        let complete = complete_link_observation("zstd", "zstd/lib");
        let link = link_record_from_observation(&complete, "x86_64-unknown-linux-gnu")?;
        let mut incomplete = RustFactWorkObservation::from(link);
        incomplete.executable = None;
        let mut observed = facts(&[], Some(retained_products()));
        observed.linked_libraries = vec!["static=zstd".to_string()];
        observed.linked_paths = vec!["native=/transient/out".to_string()];
        observed.publisher_work = vec![incomplete];
        let selected = capture(vec![(library("zstd-sys", "1.0.0", &[]), Some(observed))]);
        let release = harvest_registry_units(&selected, &evidence(), "release")?;
        let debug = harvest_registry_units(&selected, &evidence(), "debug")?;
        assert!(release.proposals.is_empty() && debug.proposals.is_empty());
        assert_eq!(release.refusals, debug.refusals);
        assert!(release.refusals.iter().any(|refusal| {
            refusal.package == "zstd-sys"
                && refusal.reason == HarvestRefusalReason::LinkedLibraries
                && !refusal.observations.link.is_empty()
        }));
        Ok(())
    }

    /// One fixture unit and, when present, the facts of the build-script edge feeding it.
    type FixtureUnit = (OvenLegacyCargoSelectedUnit, Option<OvenLegacyCargoBuildScriptFacts>);

    /// A capture of a transport root over `library` units; each unit with `Some(facts)` gets a build-script edge.
    fn capture(units: Vec<FixtureUnit>) -> OvenLegacyCargoSelectedUnitCapture {
        let mut root = library("oven_release_stdlib", "0.1.0", &[]);
        root.package_id = "path+file:///fixture#oven_release_stdlib@0.1.0".to_string();
        root.package_source = None;
        root.registry_source = None;
        let mut all = vec![root];
        for (unit, facts) in units {
            let index = all.len();
            let mut unit = unit;
            if let Some(facts) = facts {
                let script = build_script_of(&unit);
                unit.dependencies.push(OvenLegacyCargoSelectedDependency {
                    unit_index: index + 1,
                    extern_crate_name: None,
                    build_script: Some(facts),
                });
                all.push(unit);
                all.push(script);
            } else {
                all.push(unit);
            }
            let alias = all[index].package.replace('-', "_");
            all[0].dependencies.push(OvenLegacyCargoSelectedDependency {
                unit_index: index,
                extern_crate_name: Some(alias),
                build_script: None,
            });
        }
        OvenLegacyCargoSelectedUnitCapture {
            roots: vec![0],
            units: all,
            rustc_invocations_observed: true,
            build_script_tool_probes: Vec::new(),
            compiler: Some(OvenLegacyCargoSelectedCompilerContext {
                host: "x86_64-unknown-linux-gnu".to_string(),
                target: "x86_64-unknown-linux-gnu".to_string(),
                toolchain: "rustc 1.98.0 (fixture)".to_string(),
                rustc_identity: "rustc 1.98.0 (fixture)".to_string(),
                host_cfg: cfg_snapshot(),
                target_cfg: cfg_snapshot(),
            }),
        }
    }

    fn reasons(report: &HarvestReport) -> Vec<(&str, HarvestRefusalReason)> {
        report
            .refusals
            .iter()
            .map(|refusal| (refusal.package.as_str(), refusal.reason))
            .collect()
    }

    #[test]
    fn a_cfg_only_script_proposes_its_answers_and_nothing_else() -> TestResult {
        let capture = capture(vec![(
            library("libm", "0.2.16", &["default", "arch"]),
            Some(facts(
                &["optimizations_enabled", "arch_enabled", "arch_enabled"],
                Some(OvenLegacyCargoSelectedGeneratedOutput {
                    relative_root: "generated-outputs/empty".to_string(),
                    digest: selected_graph_sha256(b"empty"),
                    members: Vec::new(),
                }),
            )),
        )]);
        let report = harvest_registry_units(&capture, &evidence(), "release")?;
        assert_eq!(report.proposals.len(), 1);
        let proposal = &report.proposals[0];
        assert_eq!(proposal.project.name, "libm");
        assert_eq!(proposal.source.registry, "https://github.com/rust-lang/crates.io-index");
        assert_eq!(proposal.source.checksum, CHECKSUM);
        let fact = &proposal.rust.facts[0];
        assert_eq!(fact.features, ["arch", "default"], "features are sorted and unique");
        assert_eq!(
            fact.cfg,
            ["arch_enabled", "optimizations_enabled"],
            "cfg is sorted and unique"
        );
        assert!(fact.out.is_empty());
        assert_eq!(fact.toolchain, "rustc 1.98.0 (fixture)");
        assert_eq!(fact.profile, "release");
        assert_eq!(proposal.evidence.method, HARVEST_EVIDENCE_METHOD);
        assert_eq!(proposal.evidence.receipt, evidence().receipt);
        assert_eq!(proposal.evidence.rustc_identity, evidence().rustc_identity);
        assert_eq!(proposal.evidence.host, "x86_64-unknown-linux-gnu");
        assert!(proposal.evidence.hazards.is_empty());
        assert!(proposal.notes.is_none());
        // The transport root is refused; the build-script node is evidence owned by the library binding.
        assert_eq!(
            reasons(&report),
            [("oven_release_stdlib", HarvestRefusalReason::NotRegistryBacked)]
        );
        Ok(())
    }

    #[test]
    fn a_unit_without_a_build_script_proposes_an_empty_cfg_fact() -> TestResult {
        let mut bare = library("serde", "1.0.228", &["std"]);
        if let Some(source) = bare.registry_source.as_mut() {
            source.checksum = CHECKSUM.trim_start_matches("sha256:").to_string();
        }
        let bare_capture = capture(vec![(bare, None)]);
        let report = harvest_registry_units(&bare_capture, &evidence(), "debug")?;
        assert_eq!(report.proposals.len(), 1);
        let fact = &report.proposals[0].rust.facts[0];
        assert!(fact.cfg.is_empty() && fact.out.is_empty());
        assert_eq!(fact.profile, "debug");
        assert_eq!(
            report.proposals[0].source.checksum, CHECKSUM,
            "Cargo's bare lock checksum is proposed as the sha256 identity the registry binds"
        );
        let mut malformed = library("odd", "1.0.0", &[]);
        if let Some(source) = malformed.registry_source.as_mut() {
            source.checksum = "not-a-digest".to_string();
        }
        let report = harvest_registry_units(&capture(vec![(malformed, None)]), &evidence(), "debug")?;
        assert!(report.proposals.is_empty());
        assert_eq!(reasons(&report)[0], ("odd", HarvestRefusalReason::MalformedChecksum));
        Ok(())
    }

    #[test]
    fn out_bearing_scripts_name_nested_members_exactly_as_retained() -> TestResult {
        let capture = capture(vec![(
            library("serde_core", "1.0.228", &["std"]),
            Some(facts(
                &[],
                Some(OvenLegacyCargoSelectedGeneratedOutput {
                    relative_root: "generated-outputs/abc".to_string(),
                    digest: selected_graph_sha256(b"tree"),
                    members: vec![
                        OvenLegacyCargoInspectionSourceMember {
                            path: "private.rs".to_string(),
                            digest: selected_graph_sha256(b"private"),
                        },
                        OvenLegacyCargoInspectionSourceMember {
                            path: "nested/dir/generated.rs".to_string(),
                            digest: selected_graph_sha256(b"generated"),
                        },
                    ],
                }),
            )),
        )]);
        let report = harvest_registry_units(&capture, &evidence(), "release")?;
        let fact = &report.proposals[0].rust.facts[0];
        assert_eq!(
            fact.out
                .iter()
                .map(|out| (out.name.as_str(), out.path.as_str()))
                .collect::<Vec<_>>(),
            [
                ("nested/dir/generated.rs", "out/nested/dir/generated.rs"),
                ("private.rs", "out/private.rs"),
            ],
            "out.name is the member path the reader compares, out.path is proposal-relative"
        );
        assert_eq!(
            report.proposals[0].out_relative_root.as_deref(),
            Some("generated-outputs/abc")
        );
        Ok(())
    }

    #[test]
    fn scripts_outside_the_record_vocabulary_are_refused_by_reason() -> TestResult {
        let mut linked = facts(&[], Some(retained_products()));
        linked.linked_libraries = vec!["static=zstd".to_string()];
        let mut paths = facts(&[], Some(retained_products()));
        paths.linked_paths = vec!["/transient/out".to_string()];
        let mut environment = facts(&[], Some(retained_products()));
        environment
            .environment
            .insert("SECRET".to_string(), "ambient-value".to_string());
        let mut probed = facts(&[], Some(retained_products()));
        probed.out_dir = PathBuf::from("/transient/probed-out");
        let unretained = facts(&["answer"], None);
        let mut capture = capture(vec![
            (library("zstd-sys", "2.0.0", &[]), Some(linked)),
            (library("openssl-sys", "0.9.0", &[]), Some(paths)),
            (library("libm", "0.2.16", &[]), Some(environment)),
            (library("proc-macro2", "1.0.106", &[]), Some(probed)),
            (library("late", "1.0.0", &[]), Some(unretained)),
        ]);
        let probe_unit = capture
            .units
            .iter()
            .find(|unit| unit.package == "proc-macro2" && unit.mode == "run-custom-build")
            .ok_or("fixture must hold the proc-macro2 build script")?;
        capture
            .build_script_tool_probes
            .push(OvenLegacyCargoBuildScriptToolProbe {
                package_id: probe_unit.package_id.clone(),
                out_dir: PathBuf::from("/transient/probed-out"),
                target_context: "x86_64-unknown-linux-gnu".to_string(),
                rustc_target: "aarch64-apple-darwin".to_string(),
                digest: selected_graph_sha256(b"probe"),
                output: None,
            });
        let report = harvest_registry_units(&capture, &evidence(), "release")?;
        assert!(report.proposals.is_empty());
        let refused = report
            .refusals
            .iter()
            .filter(|refusal| refusal.reason != HarvestRefusalReason::BuildScriptUnit)
            .map(|refusal| (refusal.package.as_str(), refusal.reason, refusal.detail.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            refused,
            [
                (
                    "late",
                    HarvestRefusalReason::OutputNotRetained,
                    "OUT_DIR was not inventoried"
                ),
                ("libm", HarvestRefusalReason::EnvironmentObserved, "SECRET"),
                (
                    "openssl-sys",
                    HarvestRefusalReason::LinkedPaths,
                    "1 link search path(s)"
                ),
                (
                    "oven_release_stdlib",
                    HarvestRefusalReason::NotRegistryBacked,
                    "no package source"
                ),
                ("proc-macro2", HarvestRefusalReason::ToolProbes, "1 compiler probe(s)"),
                ("zstd-sys", HarvestRefusalReason::LinkedLibraries, "static=zstd"),
            ]
        );
        Ok(())
    }

    #[test]
    fn harvest_contract_never_emits_one_package_as_both_proposal_and_refusal() -> TestResult {
        let mut linked = facts(&[], Some(retained_products()));
        linked.linked_libraries = vec!["static=mixed".to_string()];
        let report = harvest_registry_units(
            &capture(vec![
                (library("mixed", "1.0.0", &["clean"]), None),
                (library("mixed", "1.0.0", &["native"]), Some(linked)),
                (library("plain", "1.0.0", &[]), None),
            ]),
            &evidence(),
            "release",
        )?;
        let proposed = report
            .proposals
            .iter()
            .map(|proposal| proposal.project.name.as_str())
            .collect::<BTreeSet<_>>();
        let refused = report
            .refusals
            .iter()
            .map(|refusal| refusal.package.as_str())
            .collect::<BTreeSet<_>>();
        assert!(proposed.is_disjoint(&refused));
        assert!(proposed.contains("plain"));
        assert!(!proposed.contains("mixed"));
        assert!(refused.contains("mixed"));

        let output = tempdir()?;
        let retained = tempdir()?;
        let written = write_harvest_report(&report, output.path(), retained.path())?;
        assert!(written.contains(&output.path().join("plain-1.0.0-release/proposal.json")));
        assert!(
            !written
                .iter()
                .any(|path| path.to_string_lossy().contains("mixed-1.0.0-release/proposal.json"))
        );
        let refusals: Vec<HarvestRefusal> =
            serde_json::from_slice(&fs::read(output.path().join("refusals-release.json"))?)?;
        let refused_on_disk = refusals
            .iter()
            .map(|refusal| refusal.package.as_str())
            .collect::<BTreeSet<_>>();
        assert!(proposed.is_disjoint(&refused_on_disk));
        Ok(())
    }

    #[test]
    fn a_unit_off_the_captured_target_and_a_missing_compiler_are_refused() -> TestResult {
        let mut host_only = library("host-only", "1.0.0", &[]);
        host_only.platform = Some("aarch64-apple-darwin".to_string());
        let mut capture = capture(vec![(host_only, None)]);
        let report = harvest_registry_units(&capture, &evidence(), "release")?;
        assert_eq!(reasons(&report)[0], ("host-only", HarvestRefusalReason::TargetMismatch));
        capture.compiler = None;
        assert!(harvest_registry_units(&capture, &evidence(), "release").is_err());
        assert!(harvest_registry_units(&capture, &evidence(), "bench").is_err());
        Ok(())
    }

    #[test]
    fn proposals_are_ordered_by_package_and_version_and_equal_units_fold() -> TestResult {
        let capture = capture(vec![
            (library("zeta", "1.0.0", &[]), None),
            (library("alpha", "2.0.0", &["b", "a"]), None),
            (library("alpha", "1.0.0", &[]), None),
            (library("alpha", "2.0.0", &["a", "b"]), None),
        ]);
        let report = harvest_registry_units(&capture, &evidence(), "release")?;
        assert_eq!(
            proposal_directory_names(&report)?,
            ["alpha-1.0.0-release", "alpha-2.0.0-release", "zeta-1.0.0-release"],
            "one proposal per selection, sorted; the two alpha 2.0.0 units are one selection"
        );
        Ok(())
    }

    /// Cargo orders the unit graph differently between runs; the report, refusals included, must not.
    #[test]
    fn harvest_contract_is_byte_identical_after_reordered_capture() -> TestResult {
        let empty_output = || {
            Some(OvenLegacyCargoSelectedGeneratedOutput {
                relative_root: "generated-outputs/empty".to_string(),
                digest: selected_graph_sha256(b"empty"),
                members: Vec::new(),
            })
        };
        let forward = capture(vec![
            (library("alpha", "1.0.0", &[]), Some(facts(&["one"], empty_output()))),
            (library("beta", "1.0.0", &[]), None),
            (library("gamma", "1.0.0", &[]), Some(facts(&["two"], empty_output()))),
        ]);
        let backward = capture(vec![
            (library("gamma", "1.0.0", &[]), Some(facts(&["two"], empty_output()))),
            (library("beta", "1.0.0", &[]), None),
            (library("alpha", "1.0.0", &[]), Some(facts(&["one"], empty_output()))),
        ]);
        let first = harvest_registry_units(&forward, &evidence(), "release")?;
        let second = harvest_registry_units(&backward, &evidence(), "release")?;
        assert_eq!(
            serde_json::to_vec(&first.proposals)?,
            serde_json::to_vec(&second.proposals)?
        );
        assert_eq!(
            serde_json::to_vec(&first.refusals)?,
            serde_json::to_vec(&second.refusals)?
        );
        assert!(
            first
                .refusals
                .iter()
                .all(|refusal| refusal.reason != HarvestRefusalReason::BuildScriptUnit)
        );
        Ok(())
    }

    #[test]
    fn harvest_contract_folds_equal_and_refuses_conflicting_observations() -> TestResult {
        let empty_output = || {
            Some(OvenLegacyCargoSelectedGeneratedOutput {
                relative_root: "generated-outputs/empty".to_string(),
                digest: selected_graph_sha256(b"empty"),
                members: Vec::new(),
            })
        };
        let mut one = facts(&["one"], empty_output());
        one.linked_libraries = vec!["static=one".to_string()];
        let mut two = facts(&["two"], empty_output());
        two.linked_libraries = vec!["static=two".to_string()];
        let disagreeing = capture(vec![
            (library("alpha", "1.0.0", &["x"]), Some(one)),
            (library("alpha", "1.0.0", &["x"]), Some(two)),
        ]);
        let report = harvest_registry_units(&disagreeing, &evidence(), "release")?;
        assert!(report.proposals.is_empty());
        let conflict = report
            .refusals
            .iter()
            .find(|refusal| refusal.reason == HarvestRefusalReason::ConflictingObservations)
            .ok_or("missing conflicting-observations refusal")?;
        assert_eq!(conflict.observations.link.len(), 2);

        let split = capture(vec![
            (library("syn", "2.0.0", &["full"]), None),
            (library("syn", "2.0.0", &["derive"]), None),
            (library("quote", "1.0.0", &[]), None),
        ]);
        let report = harvest_registry_units(&split, &evidence(), "release")?;
        let names = proposal_directory_names(&report)?;
        assert_eq!(names[0], "quote-1.0.0-release");
        assert!(names[1].starts_with("syn-2.0.0-release-") && names[2].starts_with("syn-2.0.0-release-"));
        assert_ne!(names[1], names[2]);
        assert_eq!(names[1].len(), "syn-2.0.0-release-".len() + 12);
        Ok(())
    }

    /// A proposal admitted into a registry checkout must be the record the reader adopts for the very capture it
    /// came from, and that record must agree with the observation `check_observation` compares it to.
    #[test]
    fn an_admitted_proposal_is_adopted_by_the_reader_for_its_own_capture() -> TestResult {
        let retained = tempdir()?;
        fs::create_dir_all(retained.path().join("generated-outputs/abc/nested"))?;
        fs::write(
            retained.path().join("generated-outputs/abc/private.rs"),
            b"pub mod private {}\n",
        )?;
        fs::write(
            retained.path().join("generated-outputs/abc/nested/generated.rs"),
            b"pub mod generated {}\n",
        )?;
        let facts = facts(
            &["if_docsrs"],
            Some(OvenLegacyCargoSelectedGeneratedOutput {
                relative_root: "generated-outputs/abc".to_string(),
                digest: selected_graph_sha256(b"tree"),
                members: vec![
                    OvenLegacyCargoInspectionSourceMember {
                        path: "private.rs".to_string(),
                        digest: digest_bytes(b"pub mod private {}\n"),
                    },
                    OvenLegacyCargoInspectionSourceMember {
                        path: "nested/generated.rs".to_string(),
                        digest: digest_bytes(b"pub mod generated {}\n"),
                    },
                ],
            }),
        );
        let capture = capture(vec![(
            library("serde_core", "1.0.228", &["std", "alloc"]),
            Some(facts.clone()),
        )]);
        let report = harvest_registry_units(&capture, &evidence(), "release")?;
        let proposal = &report.proposals[0];

        // ---- Admit the proposal the way incan-pub renders it: loaf.toml beside its out/ files, one index line ----
        let registry_root = tempdir()?;
        let record_dir = registry_root.path().join("crates-io/serde_core/1.0.228");
        write_harvest_report(&report, registry_root.path().join("harvest").as_path(), retained.path())?;
        fs::create_dir_all(&record_dir)?;
        for out in &proposal.rust.facts[0].out {
            let source = registry_root
                .path()
                .join("harvest/serde_core-1.0.228-release")
                .join(&out.path);
            let destination = record_dir.join(&out.path);
            fs::create_dir_all(destination.parent().ok_or("out path has a parent")?)?;
            fs::copy(source, destination)?;
        }
        #[derive(Serialize)]
        struct Rendered<'a> {
            project: &'a HarvestProject,
            source: &'a HarvestSource,
            rust: RenderedRust,
        }
        #[derive(Serialize)]
        struct RenderedRust {
            facts: Vec<RustFactRecord>,
        }
        fs::write(
            record_dir.join("loaf.toml"),
            toml::to_string(&Rendered {
                project: &proposal.project,
                source: &proposal.source,
                rust: RenderedRust {
                    facts: vec![proposal.admitted_record()?],
                },
            })?,
        )?;
        let index_dir = registry_root.path().join("index/se/rd");
        fs::create_dir_all(&index_dir)?;
        fs::write(
            index_dir.join("serde_core"),
            format!(
                "{{\"cksum\":\"{CHECKSUM}\",\"manifest\":\"crates-io/serde_core/1.0.228/loaf.toml\",\"name\":\"serde_core\",\"source\":\"crates-io\",\"vers\":\"1.0.228\"}}\n"
            ),
        )?;

        // ---- The reader adopts the library unit and its declaration agrees with the observation ----
        let registry = oven_model::loaf_registry::LoafRegistry::open(registry_root.path())?;
        let authority = super::super::LoafRegistryAuthority::resolve(&capture, &registry, "release")?;
        let adoption = authority.adoption(1).ok_or("the harvested unit must be adopted")?;
        assert_eq!(adoption.checksum, CHECKSUM);
        super::super::LoafRegistryAuthority::check_observation(adoption, Some(&facts), false)?;
        assert!(
            super::super::LoafRegistryAuthority::resolve(&capture, &registry, "debug")?.is_empty(),
            "the record binds the harvested profile only"
        );
        Ok(())
    }

    /// Every hazard token admission refuses on is recorded, never refused on, and spelled exactly.
    #[test]
    fn hazards_record_the_publisher_environment_without_refusing() -> TestResult {
        let mut nightly = capture(vec![(library("serde", "1.0.228", &[]), None)]);
        if let Some(compiler) = nightly.compiler.as_mut() {
            compiler.toolchain = "rustc 1.99.0-nightly (abcdef123 2026-03-24)".to_string();
            compiler.rustc_identity = compiler.toolchain.clone();
        }
        let mut hazardous = evidence();
        hazardous.ambient_hazards = vec![HARVEST_HAZARD_RUSTC_BOOTSTRAP.to_string()];
        hazardous.cargo_version = "cargo 1.99.0-nightly (123abc 2026-03-24)".to_string();
        let report = harvest_registry_units(&nightly, &hazardous, "release")?;
        assert_eq!(
            report.proposals.len(),
            1,
            "a hazard never refuses; the registry decides"
        );
        assert_eq!(report.hazards, report.proposals[0].evidence.hazards);
        assert_eq!(
            report.proposals[0].evidence.hazards,
            ["RUSTC_BOOTSTRAP", "nightly-rustc"],
            "sorted, unique, and spelled as admission expects"
        );
        // The publisher Cargo is nightly by design (`--unit-graph`); it compiles nothing, so it is provenance in
        // `cargo_version`, not a hazard.
        let mut stable_rustc_nightly_cargo = evidence();
        stable_rustc_nightly_cargo.cargo_version = "cargo 1.99.0-nightly (123abc 2026-03-24)".to_string();
        let capture = capture(vec![(library("serde", "1.0.228", &[]), None)]);
        let report = harvest_registry_units(&capture, &stable_rustc_nightly_cargo, "release")?;
        assert!(report.proposals[0].evidence.hazards.is_empty());
        assert!(report.proposals[0].evidence.cargo_version.contains("nightly"));
        assert_eq!(
            ambient_harvest_hazards().len(),
            usize::from(std::env::var_os("RUSTC_BOOTSTRAP").is_some())
        );
        Ok(())
    }

    /// `evidence.receipt` is written in the one spelling admission accepts or not at all; `notes` only when known.
    #[test]
    fn receipt_and_notes_are_written_only_in_admissible_shapes() -> TestResult {
        let capture = capture(vec![(library("serde", "1.0.228", &[]), None)]);
        let identity = HarvestPublisherIdentity::new(
            "receipt-without-a-digest",
            &format!("sha256:{}", "4".repeat(64)),
            Vec::new(),
            Some(harvest_notes_for_checkout("8d40e1d3e5c43139a11b406dd0ba6e092efac492")),
        );
        assert!(identity.receipt.is_none(), "another spelling is dropped, not rewritten");
        let mut inputs = evidence();
        inputs.receipt = identity.receipt;
        inputs.notes = identity.notes;
        let report = harvest_registry_units(&capture, &inputs, "release")?;
        let json = serde_json::to_value(&report.proposals[0])?;
        assert!(json["evidence"].get("receipt").is_none());
        assert_eq!(json["notes"], "harvested from the release capture at 8d40e1d");
        let kept = HarvestPublisherIdentity::new(&format!("sha256:{}", "1".repeat(64)), "x", Vec::new(), None);
        assert!(kept.receipt.is_some());
        let json = serde_json::to_value(&harvest_registry_units(&capture, &evidence(), "release")?.proposals[0])?;
        assert_eq!(json["evidence"]["receipt"], format!("sha256:{}", "1".repeat(64)));
        assert!(json.get("notes").is_none());
        Ok(())
    }

    /// A fact carries exactly the record vocabulary, and an `out` entry names a committed file safely and once.
    #[test]
    fn a_fact_carries_exactly_the_record_keys_and_safe_unique_outputs() -> TestResult {
        let capture = capture(vec![(
            library("serde_core", "1.0.228", &[]),
            Some(facts(
                &["answer"],
                Some(OvenLegacyCargoSelectedGeneratedOutput {
                    relative_root: "generated-outputs/abc".to_string(),
                    digest: selected_graph_sha256(b"tree"),
                    members: vec![OvenLegacyCargoInspectionSourceMember {
                        path: "private.rs".to_string(),
                        digest: selected_graph_sha256(b"private"),
                    }],
                }),
            )),
        )]);
        let json = serde_json::to_value(&harvest_registry_units(&capture, &evidence(), "release")?.proposals[0])?;
        let keys = json["rust"]["facts"][0]
            .as_object()
            .ok_or("fact must be an object")?
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(keys, ["cfg", "features", "out", "profile", "target", "toolchain"]);
        assert_eq!(json["rust"]["facts"][0]["out"][0]["path"], "out/private.rs");

        let escaping = capture_with_members(vec![OvenLegacyCargoInspectionSourceMember {
            path: "../escape.rs".to_string(),
            digest: selected_graph_sha256(b"x"),
        }]);
        let report = harvest_registry_units(&escaping, &evidence(), "release")?;
        assert!(report.proposals.is_empty());
        assert!(
            report
                .refusals
                .iter()
                .any(|refusal| refusal.reason == HarvestRefusalReason::MalformedOutput)
        );
        let repeated = capture_with_members(vec![
            OvenLegacyCargoInspectionSourceMember {
                path: "twice.rs".to_string(),
                digest: selected_graph_sha256(b"a"),
            },
            OvenLegacyCargoInspectionSourceMember {
                path: "twice.rs".to_string(),
                digest: selected_graph_sha256(b"b"),
            },
        ]);
        let report = harvest_registry_units(&repeated, &evidence(), "release")?;
        assert!(report.proposals.is_empty());
        assert!(
            report
                .refusals
                .iter()
                .any(|refusal| refusal.reason == HarvestRefusalReason::MalformedOutput
                    && refusal.detail.contains("twice"))
        );
        Ok(())
    }

    /// A registry library whose script retained exactly `members`.
    fn capture_with_members(members: Vec<OvenLegacyCargoInspectionSourceMember>) -> OvenLegacyCargoSelectedUnitCapture {
        capture(vec![(
            library("odd", "1.0.0", &[]),
            Some(facts(
                &[],
                Some(OvenLegacyCargoSelectedGeneratedOutput {
                    relative_root: "generated-outputs/odd".to_string(),
                    digest: selected_graph_sha256(b"odd"),
                    members,
                }),
            )),
        )])
    }

    #[test]
    fn the_written_report_is_canonical_and_idempotent() -> TestResult {
        let retained = tempdir()?;
        let member_root = retained.path().join("generated-outputs/abc/nested");
        fs::create_dir_all(&member_root)?;
        fs::write(member_root.join("generated.rs"), b"pub mod generated {}\n")?;
        fs::write(
            retained.path().join("generated-outputs/abc/private.rs"),
            b"pub mod private {}\n",
        )?;
        let capture = capture(vec![
            (
                library("serde_core", "1.0.228", &["std"]),
                Some(facts(
                    &["if_docsrs"],
                    Some(OvenLegacyCargoSelectedGeneratedOutput {
                        relative_root: "generated-outputs/abc".to_string(),
                        digest: selected_graph_sha256(b"tree"),
                        members: vec![
                            OvenLegacyCargoInspectionSourceMember {
                                path: "private.rs".to_string(),
                                digest: digest_bytes(b"pub mod private {}\n"),
                            },
                            OvenLegacyCargoInspectionSourceMember {
                                path: "nested/generated.rs".to_string(),
                                digest: digest_bytes(b"pub mod generated {}\n"),
                            },
                        ],
                    }),
                )),
            ),
            (library("quote", "1.0.0", &[]), None),
        ]);
        let report = harvest_registry_units(&capture, &evidence(), "release")?;
        let output = tempdir()?;
        let written = write_harvest_report(&report, output.path(), retained.path())?;
        assert_eq!(
            written,
            [
                output.path().join("quote-1.0.0-release/proposal.json"),
                output.path().join("serde_core-1.0.228-release/out/nested/generated.rs"),
                output.path().join("serde_core-1.0.228-release/out/private.rs"),
                output.path().join("serde_core-1.0.228-release/proposal.json"),
                output.path().join("refusals-release.json"),
            ]
        );
        let proposal_text = fs::read_to_string(output.path().join("serde_core-1.0.228-release/proposal.json"))?;
        let proposal: serde_json::Value = serde_json::from_str(&proposal_text)?;
        let keys = proposal
            .as_object()
            .ok_or("proposal must be an object")?
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(keys, ["evidence", "project", "rust", "source"], "keys are sorted");
        assert!(proposal_text.ends_with('\n'));
        assert_eq!(proposal["rust"]["facts"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            proposal["rust"]["facts"][0]["out"][0]["path"],
            "out/nested/generated.rs"
        );
        assert!(
            proposal["rust"]["facts"][0].get("harvested-from").is_none()
                && proposal["rust"]["facts"][0].get("link").is_none(),
            "the proposal never carries the keys admission fills or reserves"
        );
        assert_eq!(proposal["evidence"]["method"], HARVEST_EVIDENCE_METHOD);
        assert_eq!(proposal["evidence"]["hazards"], serde_json::json!([]));
        assert_eq!(proposal["evidence"]["host"], "x86_64-unknown-linux-gnu");
        let round_trip: HarvestProposal = serde_json::from_str(&proposal_text)?;
        assert_eq!(round_trip.rust, report.proposals[1].rust);
        assert_eq!(
            fs::read(output.path().join("serde_core-1.0.228-release/out/private.rs"))?,
            b"pub mod private {}\n"
        );
        let refusals: Vec<HarvestRefusal> =
            serde_json::from_slice(&fs::read(output.path().join("refusals-release.json"))?)?;
        assert_eq!(refusals, report.refusals);
        assert!(
            refusals.iter().all(|refusal| refusal.package != "serde_core"),
            "serde_core's build-script node is supporting evidence for its proposal, never a refusal of the package"
        );
        assert_eq!(
            serde_json::to_value(HarvestRefusalReason::BuildScriptUnit)?,
            "build-script-unit",
            "reasons are kebab-case on the wire"
        );

        // ---- Idempotence: the same report rewrites nothing and refuses a changed answer ----
        let again = write_harvest_report(&report, output.path(), retained.path())?;
        assert_eq!(again, written);
        // ---- The other profile's harvest shares the directory without contending for the refusal list ----
        let debug = harvest_registry_units(&capture, &evidence(), "debug")?;
        let debug_written = write_harvest_report(&debug, output.path(), retained.path())?;
        assert!(debug_written.contains(&output.path().join("serde_core-1.0.228-debug/proposal.json")));
        assert!(debug_written.contains(&output.path().join("refusals-debug.json")));
        // ---- Provenance moves with every commit; the binding does not, and that is what idempotence is over ----
        let mut later = report.clone();
        later.proposals[0].notes = Some("harvested from the release capture at 1234567".to_string());
        later.proposals[0].evidence.receipt = Some(format!("sha256:{}", "9".repeat(64)));
        let rewritten = write_harvest_report(&later, output.path(), retained.path())?;
        assert_eq!(rewritten, written);
        assert_eq!(
            fs::read_to_string(output.path().join("quote-1.0.0-release/proposal.json"))?,
            fs::read_to_string(output.path().join("quote-1.0.0-release/proposal.json"))?,
            "the earlier observation stays as written"
        );
        // The changed answer also retains one more member; the refusal must come before that member is copied, so
        // the directory stays exactly what the earlier harvest wrote.
        fs::write(
            retained.path().join("generated-outputs/abc/extra.rs"),
            b"pub mod extra {}\n",
        )?;
        let mut changed = report.clone();
        changed.proposals[1].rust.facts[0].cfg.push("new_answer".to_string());
        changed.proposals[1].rust.facts[0].out.push(RustFactOut {
            name: "extra.rs".to_string(),
            path: "out/extra.rs".to_string(),
            digest: digest_bytes(b"pub mod extra {}\n"),
        });
        let refused = write_harvest_report(&changed, output.path(), retained.path());
        assert!(
            refused
                .as_ref()
                .err()
                .is_some_and(|error| error.to_string().contains("binds different facts")),
            "a changed answer for the same directory is refused: {:?}",
            refused.as_ref().err().map(ToString::to_string)
        );
        assert!(
            !output.path().join("serde_core-1.0.228-release/out/extra.rs").exists(),
            "a refused rewrite copies no member the earlier proposal does not name"
        );

        // ---- A retained member that no longer matches its digest is refused before anything is copied ----
        fs::write(retained.path().join("generated-outputs/abc/private.rs"), b"tampered\n")?;
        let fresh = tempdir()?;
        assert!(write_harvest_report(&report, fresh.path(), retained.path()).is_err());
        Ok(())
    }
}
