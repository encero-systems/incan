//! Canonical harvest serialization, proposal naming, and idempotent report persistence.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use oven_model::digest::canonical_json_bytes_pretty;
use serde::Serialize;

use super::super::{OvenLegacyCargoError, OvenLegacyCargoSelectedUnitCapture, digest_bytes, regular_file_bytes};
use super::model::{HARVEST_PROPOSAL_FILE, HarvestEvidenceInputs, HarvestProposal, HarvestRefusal, HarvestReport};
use super::observe::harvest_registry_units;

/// File name of the refusal list written beside one profile's proposal directories.
pub fn harvest_refusals_file_name(profile: &str) -> String {
    format!("refusals-{profile}.json")
}

// ============================================================================
// Writing a report
// ============================================================================

/// Canonical bytes of one proposal: sorted keys, two-space indentation, trailing newline.
///
/// The model-owned canonical encoder sorts every object explicitly, so the result is independent of the JSON map
/// implementation selected elsewhere in the workspace and comparable byte-for-byte with what is on disk.
pub fn canonical_proposal_bytes(proposal: &HarvestProposal) -> Result<Vec<u8>, OvenLegacyCargoError> {
    canonical_json_bytes(proposal)
}

/// Canonical bytes of the refusal list.
pub fn canonical_refusals_bytes(refusals: &[HarvestRefusal]) -> Result<Vec<u8>, OvenLegacyCargoError> {
    canonical_json_bytes(refusals)
}

/// Sorted-key pretty JSON with a trailing newline.
fn canonical_json_bytes<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, OvenLegacyCargoError> {
    let mut bytes = canonical_json_bytes_pretty(value)
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
pub(super) fn safe_relative(value: &str) -> Option<PathBuf> {
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
