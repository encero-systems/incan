//! Harvest: what the compatibility publisher observed for each registry unit, written as incan.pub proposals.
//!
//! RFC 119 makes adoption a harvest, not a hand edit: a crates.io package gains its declared build facts by running
//! one coordinated Cargo build for the selected closure on the publisher's machine and reading Cargo's build-script
//! output records into `cfg` and `out`. The capture that build leaves behind (`OvenLegacyCargoSelectedUnitCapture`)
//! is the observation; this module turns it into one proposal per registry package, version and exact selection,
//! and one refusal per unit whose observation the record vocabulary cannot carry. Admitting a proposal is
//! `incan-pub add-fact`'s job, in a separate, reviewable step; nothing here writes into a registry.
//!
//! Every fact proposed here is one `LoafRegistryAuthority::resolve` will later compare with a fresh observation
//! (`check_observation`), so the shape emitted must be the shape the reader compares: `features` and `cfg` sorted and
//! unique, `out.name` exactly the retained member path, `out.path` relative to the proposal file.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use oven_model::manifest::RustFactOut;
use serde::{Deserialize, Serialize};

use super::{
    OvenLegacyCargoBuildScriptFacts, OvenLegacyCargoError, OvenLegacyCargoPrepareResult,
    OvenLegacyCargoSelectedCompilerContext, OvenLegacyCargoSelectedUnit, OvenLegacyCargoSelectedUnitCapture,
    digest_bytes, regular_file_bytes,
};

/// The `evidence.method` every proposal records: the facts come from watching Cargo, not from reading a manifest.
pub const HARVEST_EVIDENCE_METHOD: &str = "compatibility publisher observation";

/// File name of the refusal list `write_harvest_report` writes beside the proposal directories.
pub const HARVEST_REFUSALS_FILE: &str = "refusals.json";

/// File name of the proposal inside each `<name>-<version>` directory.
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

/// One proposed `[[rust.facts]]` record: the `RustFactRecord` shape minus the keys admission fills or reserves.
///
/// `harvested-from` is derived by admission from `evidence.receipt`; `link` and `tool` are reserved, and a unit that
/// would need them is refused rather than proposed.
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
    /// Retained `OUT_DIR` members: `name` is the member path, `path` is `out/<name>` relative to the proposal.
    pub out: Vec<RustFactOut>,
}

/// `rust`: the fact list, which a proposal keeps to exactly one entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarvestRustFacts {
    /// Exactly one record per proposal.
    pub facts: Vec<HarvestFact>,
}

/// `evidence`: what the observation ran under. Admission stores `receipt` as `harvested-from` and the rest verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarvestEvidence {
    /// Always [`HARVEST_EVIDENCE_METHOD`].
    pub method: String,
    /// `sha256:` identity of the compatibility receipt whose capture proposed the record.
    pub receipt: String,
    /// `cargo --version` as the publisher observed it.
    pub cargo_version: String,
    /// `sha256:` digest of the publisher's `Cargo.lock`.
    pub cargo_lock_digest: String,
    /// `sha256:` digest of the publisher's `Cargo.toml`.
    pub cargo_manifest_digest: String,
}

/// One harvest proposal, as `incan-pub add-fact` admits it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarvestProposal {
    pub project: HarvestProject,
    pub source: HarvestSource,
    pub rust: HarvestRustFacts,
    pub evidence: HarvestEvidence,
    /// Where the retained `OUT_DIR` tree of this fact lives, relative to the retention root the writer is given.
    ///
    /// Never serialized: it is a publisher-local coordinate, and the proposal names its inputs by digest.
    #[serde(skip)]
    pub out_relative_root: Option<String>,
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
    /// The script linked libraries, which the reserved `link` grammar must carry.
    LinkedLibraries,
    /// The script emitted link search paths, which the reserved `link` grammar must carry.
    LinkedPaths,
    /// The script ran compiler probes, which the reserved `tool` grammar must carry.
    ToolProbes,
    /// The script emitted `rustc-env` values, which Cargo set on the consumer's compilation and no record key
    /// carries.
    EnvironmentObserved,
    /// The script's `OUT_DIR` was never retained, so its members cannot be named by digest.
    OutputNotRetained,
    /// The unit compiled for a platform other than the captured target, which the reader binds every record to.
    TargetMismatch,
    /// More than one build-script edge feeds the unit; the observation is not one script's answer.
    MultipleBuildScriptEdges,
    /// Two units of the same package, version and selection observed different facts.
    ConflictingObservations,
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
}

/// Everything one harvest produced: proposals and refusals, each in deterministic order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HarvestReport {
    /// Proposals ordered by package, version, then selection.
    pub proposals: Vec<HarvestProposal>,
    /// Refusals ordered by package, version, reason, detail.
    pub refusals: Vec<HarvestRefusal>,
}

/// The publisher facts a proposal records as evidence, taken from the preparation that produced the capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarvestEvidenceInputs {
    /// `sha256:` identity of the compatibility receipt.
    pub receipt: String,
    /// Cargo's version string.
    pub cargo_version: String,
    /// Digest of the publisher `Cargo.lock`.
    pub cargo_lock_digest: String,
    /// Digest of the publisher `Cargo.toml`.
    pub cargo_manifest_digest: String,
}

impl HarvestEvidenceInputs {
    /// Evidence for the capture one `prepare_direct_rustc_plan` call produced, under the receipt that authorized it.
    pub fn from_prepare_result(prepared: &OvenLegacyCargoPrepareResult, receipt: &str) -> Self {
        Self {
            receipt: receipt.to_string(),
            cargo_version: prepared.cargo_version.clone(),
            cargo_lock_digest: prepared.cargo_lock_digest.clone(),
            cargo_manifest_digest: prepared.cargo_manifest_digest.clone(),
        }
    }
}

// ============================================================================
// Harvesting a capture
// ============================================================================

/// The observation of one registry unit before it is folded with its equals.
struct Observation {
    fact: HarvestFact,
    source: HarvestSource,
    out_relative_root: Option<String>,
}

/// Turn one capture into proposals for every adoptable registry unit and refusals for the rest.
///
/// A unit is adoptable when it is registry-backed, compiled for the captured target, and either has no build script
/// or has exactly one whose script emitted only `cfg` answers and `OUT_DIR` members, both possibly empty. The
/// proposal's `out` entries name every retained member by its path and digest, so the reader's
/// `check_observation` compares equal on the next capture. Everything else is a refusal with a typed reason.
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
    let mut refusals = BTreeSet::new();
    let mut observations: BTreeMap<(String, String, Vec<String>), Vec<Observation>> = BTreeMap::new();

    // ---- Classify every unit: refuse, or record its observation ----
    for (index, unit) in capture.units.iter().enumerate() {
        match observe_unit(capture, compiler, unit, index, profile) {
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
            refusals.insert(HarvestRefusal {
                package,
                version,
                reason: HarvestRefusalReason::ConflictingObservations,
                detail: format!("{} units of one selection observed different facts", group.len() + 1),
            });
            continue;
        }
        proposals.push(HarvestProposal {
            project: HarvestProject { name: package, version },
            source: first.source,
            rust: HarvestRustFacts {
                facts: vec![first.fact],
            },
            evidence: HarvestEvidence {
                method: HARVEST_EVIDENCE_METHOD.to_string(),
                receipt: evidence.receipt.clone(),
                cargo_version: evidence.cargo_version.clone(),
                cargo_lock_digest: evidence.cargo_lock_digest.clone(),
                cargo_manifest_digest: evidence.cargo_manifest_digest.clone(),
            },
            out_relative_root: first.out_relative_root,
        });
    }
    Ok(HarvestReport {
        proposals,
        refusals: refusals.into_iter().collect(),
    })
}

/// Observe one unit, or say why it cannot be proposed.
fn observe_unit(
    capture: &OvenLegacyCargoSelectedUnitCapture,
    compiler: &OvenLegacyCargoSelectedCompilerContext,
    unit: &OvenLegacyCargoSelectedUnit,
    index: usize,
    profile: &str,
) -> Result<Observation, HarvestRefusal> {
    let refuse = |reason: HarvestRefusalReason, detail: String| HarvestRefusal {
        package: unit.package.clone(),
        version: unit.package_version.clone(),
        reason,
        detail,
    };
    if is_build_script_unit(unit) {
        return Err(refuse(
            HarvestRefusalReason::BuildScriptUnit,
            format!("run-custom-build unit {index}"),
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
            // ---- Anything outside cfg and OUT_DIR needs a grammar that does not exist yet ----
            if !facts.linked_libraries.is_empty() {
                return Err(refuse(
                    HarvestRefusalReason::LinkedLibraries,
                    facts.linked_libraries.join(", "),
                ));
            }
            if !facts.linked_paths.is_empty() {
                return Err(refuse(
                    HarvestRefusalReason::LinkedPaths,
                    format!("{} link search path(s)", facts.linked_paths.len()),
                ));
            }
            let probes = capture
                .build_script_tool_probes
                .iter()
                .filter(|probe| probe.package_id == build_unit.package_id && probe.out_dir == facts.out_dir)
                .count();
            if probes > 0 {
                return Err(refuse(
                    HarvestRefusalReason::ToolProbes,
                    format!("{probes} compiler probe(s)"),
                ));
            }
            if !facts.environment.is_empty() {
                return Err(refuse(
                    HarvestRefusalReason::EnvironmentObserved,
                    facts.environment.keys().cloned().collect::<Vec<_>>().join(", "),
                ));
            }
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
    let mut out = output
        .map(|output| {
            output
                .members
                .iter()
                .map(|member| RustFactOut {
                    name: member.path.clone(),
                    path: format!("{HARVEST_OUT_DIRECTORY}/{}", member.path),
                    digest: member.digest.clone(),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    out.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(Observation {
        fact: HarvestFact {
            toolchain: compiler.toolchain.clone(),
            target: compiler.target.clone(),
            profile: profile.to_string(),
            features,
            cfg,
            out,
        },
        source: HarvestSource {
            registry: registry_index_of(&registry_source.registry),
            checksum: registry_source.checksum.clone(),
        },
        out_relative_root: output.map(|output| output.relative_root.clone()),
    })
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
/// `<name>-<version>` when the report holds one selection for that package version. A capture can hold more than
/// one (Cargo's resolver keeps host and target feature sets apart), and then every directory for that package
/// version carries a short digest of its selection so the names stay stable whichever other selections appear.
pub fn proposal_directory_names(report: &HarvestReport) -> Result<Vec<String>, OvenLegacyCargoError> {
    let mut per_version = BTreeMap::<(&str, &str), usize>::new();
    for proposal in &report.proposals {
        *per_version
            .entry((&proposal.project.name, &proposal.project.version))
            .or_default() += 1;
    }
    report
        .proposals
        .iter()
        .map(|proposal| {
            let base = format!("{}-{}", proposal.project.name, proposal.project.version);
            let count = per_version
                .get(&(proposal.project.name.as_str(), proposal.project.version.as_str()))
                .copied()
                .unwrap_or(1);
            if count == 1 {
                return Ok(base);
            }
            let fact =
                proposal.rust.facts.first().ok_or_else(|| {
                    OvenLegacyCargoError::Plan(format!("harvest proposal for `{base}` carries no fact"))
                })?;
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

/// Write the report under `dir`: one `<name>-<version>/proposal.json` per proposal with its `out/` members copied
/// beside it, and `refusals.json` at the root. Returns every file written, in order.
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
        let proposal_path = proposal_dir.join(HARVEST_PROPOSAL_FILE);
        write_idempotently(&proposal_path, &canonical_proposal_bytes(proposal)?, &name)?;
        written.push(proposal_path);
    }
    let refusals_path = dir.join(HARVEST_REFUSALS_FILE);
    write_idempotently(
        &refusals_path,
        &canonical_refusals_bytes(&report.refusals)?,
        HARVEST_REFUSALS_FILE,
    )?;
    written.push(refusals_path);
    Ok(written)
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
            receipt: format!("sha256:{}", "1".repeat(64)),
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
        }
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
        // The transport root and the run-custom-build node are refused by name, not silently dropped.
        assert_eq!(
            reasons(&report),
            [
                ("libm", HarvestRefusalReason::BuildScriptUnit),
                ("oven_release_stdlib", HarvestRefusalReason::NotRegistryBacked),
            ]
        );
        Ok(())
    }

    #[test]
    fn a_unit_without_a_build_script_proposes_an_empty_cfg_fact() -> TestResult {
        let capture = capture(vec![(library("serde", "1.0.228", &["std"]), None)]);
        let report = harvest_registry_units(&capture, &evidence(), "debug")?;
        assert_eq!(report.proposals.len(), 1);
        let fact = &report.proposals[0].rust.facts[0];
        assert!(fact.cfg.is_empty() && fact.out.is_empty());
        assert_eq!(fact.profile, "debug");
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
        let mut linked = facts(&[], None);
        linked.linked_libraries = vec!["static=zstd".to_string()];
        let mut paths = facts(&[], None);
        paths.linked_paths = vec!["/transient/out/lib".to_string()];
        let mut environment = facts(&[], None);
        environment
            .environment
            .insert("CFG_OPT_LEVEL".to_string(), "3".to_string());
        let mut probed = facts(&[], None);
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
                rustc_target: "x86_64-unknown-linux-gnu".to_string(),
                digest: selected_graph_sha256(b"probe"),
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
                ("libm", HarvestRefusalReason::EnvironmentObserved, "CFG_OPT_LEVEL"),
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
            report
                .proposals
                .iter()
                .map(|proposal| format!("{}-{}", proposal.project.name, proposal.project.version))
                .collect::<Vec<_>>(),
            ["alpha-1.0.0", "alpha-2.0.0", "zeta-1.0.0"],
            "one proposal per selection, sorted; the two alpha 2.0.0 units are one selection"
        );
        Ok(())
    }

    #[test]
    fn units_of_one_selection_that_disagree_refuse_and_distinct_selections_get_distinct_directories() -> TestResult {
        let empty_output = || {
            Some(OvenLegacyCargoSelectedGeneratedOutput {
                relative_root: "generated-outputs/empty".to_string(),
                digest: selected_graph_sha256(b"empty"),
                members: Vec::new(),
            })
        };
        let disagreeing = capture(vec![
            (library("alpha", "1.0.0", &["x"]), Some(facts(&["one"], empty_output()))),
            (library("alpha", "1.0.0", &["x"]), Some(facts(&["two"], empty_output()))),
        ]);
        let report = harvest_registry_units(&disagreeing, &evidence(), "release")?;
        assert!(report.proposals.is_empty());
        assert!(
            report
                .refusals
                .iter()
                .any(|refusal| refusal.reason == HarvestRefusalReason::ConflictingObservations)
        );

        let split = capture(vec![
            (library("syn", "2.0.0", &["full"]), None),
            (library("syn", "2.0.0", &["derive"]), None),
            (library("quote", "1.0.0", &[]), None),
        ]);
        let report = harvest_registry_units(&split, &evidence(), "release")?;
        let names = proposal_directory_names(&report)?;
        assert_eq!(names[0], "quote-1.0.0");
        assert!(names[1].starts_with("syn-2.0.0-") && names[2].starts_with("syn-2.0.0-"));
        assert_ne!(names[1], names[2]);
        assert_eq!(names[1].len(), "syn-2.0.0-".len() + 12);
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
            let source = registry_root.path().join("harvest/serde_core-1.0.228").join(&out.path);
            let destination = record_dir.join(&out.path);
            fs::create_dir_all(destination.parent().ok_or("out path has a parent")?)?;
            fs::copy(source, destination)?;
        }
        #[derive(Serialize)]
        struct Rendered<'a> {
            project: &'a HarvestProject,
            source: &'a HarvestSource,
            rust: &'a HarvestRustFacts,
        }
        fs::write(
            record_dir.join("loaf.toml"),
            toml::to_string(&Rendered {
                project: &proposal.project,
                source: &proposal.source,
                rust: &proposal.rust,
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
        super::super::LoafRegistryAuthority::check_observation(adoption, Some(&facts))?;
        assert!(
            super::super::LoafRegistryAuthority::resolve(&capture, &registry, "debug")?.is_empty(),
            "the record binds the harvested profile only"
        );
        Ok(())
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
                output.path().join("quote-1.0.0/proposal.json"),
                output.path().join("serde_core-1.0.228/out/nested/generated.rs"),
                output.path().join("serde_core-1.0.228/out/private.rs"),
                output.path().join("serde_core-1.0.228/proposal.json"),
                output.path().join("refusals.json"),
            ]
        );
        let proposal_text = fs::read_to_string(output.path().join("serde_core-1.0.228/proposal.json"))?;
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
        let round_trip: HarvestProposal = serde_json::from_str(&proposal_text)?;
        assert_eq!(round_trip.rust, report.proposals[1].rust);
        assert_eq!(
            fs::read(output.path().join("serde_core-1.0.228/out/private.rs"))?,
            b"pub mod private {}\n"
        );
        let refusals: Vec<HarvestRefusal> = serde_json::from_slice(&fs::read(output.path().join("refusals.json"))?)?;
        assert_eq!(refusals, report.refusals);
        let script_refusal = refusals
            .iter()
            .find(|refusal| refusal.package == "serde_core")
            .ok_or("the serde_core build script is refused by name")?;
        assert_eq!(
            serde_json::to_value(script_refusal.reason)?,
            "build-script-unit",
            "reasons are kebab-case on the wire"
        );

        // ---- Idempotence: the same report rewrites nothing and refuses a changed answer ----
        let again = write_harvest_report(&report, output.path(), retained.path())?;
        assert_eq!(again, written);
        let mut changed = report.clone();
        changed.proposals[0].rust.facts[0].cfg.push("new_answer".to_string());
        let refused = write_harvest_report(&changed, output.path(), retained.path());
        assert!(
            refused
                .as_ref()
                .err()
                .is_some_and(|error| error.to_string().contains("different bytes")),
            "a changed answer for the same directory is refused: {:?}",
            refused.as_ref().err().map(ToString::to_string)
        );

        // ---- A retained member that no longer matches its digest is refused before anything is copied ----
        fs::write(retained.path().join("generated-outputs/abc/private.rs"), b"tampered\n")?;
        let fresh = tempdir()?;
        assert!(write_harvest_report(&report, fresh.path(), retained.path()).is_err());
        Ok(())
    }
}
