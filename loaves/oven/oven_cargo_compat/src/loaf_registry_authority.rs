//! Declared build facts a registered Loaf registry supplies for captured units.
//!
//! The compatibility publisher observes what each build script emitted. Where a registry publishes an adoption
//! manifest for the exact captured source and selection, that declaration is the unit's authority under RFC 119:
//! the projection carries the declared facts and nothing a script emitted outside them. While both exist, the
//! declaration and the observation must agree on every fact they both state, so a stale or wrong record is a
//! refusal rather than a quiet substitution.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use oven_model::loaf_registry::LoafRegistry;
use oven_model::lock::RegistryRecord;
use oven_model::manifest::{RustFactRecord, RustFactSelection};

use super::{OvenLegacyCargoBuildScriptFacts, OvenLegacyCargoError, OvenLegacyCargoSelectedUnitCapture};
use crate::harvest::{HARVEST_PROPOSAL_FILE, HarvestProposal};

/// One adopted captured unit and the record that governs it.
#[derive(Debug, Clone)]
pub struct LoafRegistryAdoption {
    /// Package name as the registry publishes it.
    pub package: String,
    /// Exact version.
    pub version: String,
    /// Canonical checksum of the source both sides describe.
    pub checksum: String,
    /// Identity of the exact index line the package was selected from.
    pub index_line_digest: String,
    /// Content identity of the exact governing manifest bytes.
    pub manifest_digest: String,
    /// Immutable registry directory that owns the record's declared source and tool-input closure.
    pub manifest_root: PathBuf,
    /// `harvested` or `attested`, as the index line states it for this binding.
    pub status: String,
    /// The bound record.
    pub record: RustFactRecord,
}

/// Adopted units of one capture, keyed by capture unit index.
#[derive(Debug, Clone, Default)]
pub struct LoafRegistryAuthority {
    adoptions: BTreeMap<usize, LoafRegistryAdoption>,
}

impl LoafRegistryAuthority {
    /// An authority that adopts nothing: every unit keeps its observed facts.
    pub fn none() -> Self {
        Self::default()
    }

    /// Resolve every registry-backed captured unit against the registry for the capture's exact selection.
    ///
    /// A package the registry does not describe, or describes without a record bound to this selection, is simply
    /// not adopted. A package it describes for a different source is a refusal inside the registry lookup.
    pub fn resolve(
        capture: &OvenLegacyCargoSelectedUnitCapture,
        registry: &LoafRegistry,
        profile: &str,
    ) -> Result<Self, OvenLegacyCargoError> {
        let compiler = capture.compiler.as_ref().ok_or_else(|| {
            OvenLegacyCargoError::Plan("Loaf registry adoption requires a captured compiler selection".to_string())
        })?;
        let mut adoptions = BTreeMap::new();
        for (index, unit) in capture.units.iter().enumerate() {
            if unit.mode == "run-custom-build" {
                continue;
            }
            let Some(registry_source) = unit.registry_source.as_ref() else {
                continue;
            };
            let package = registry
                .package(&unit.package, &unit.package_version, &registry_source.checksum)
                .map_err(|error| OvenLegacyCargoError::Plan(error.to_string()))?;
            let Some(package) = package else {
                continue;
            };
            let Some(platform) = unit.platform.as_ref() else {
                continue;
            };
            let selection = RustFactSelection {
                toolchain: compiler.toolchain.clone(),
                target: platform.clone(),
                profile: profile.to_string(),
                features: unit.effective_features.clone(),
            };
            let Some(record) = package.fact_record(&selection) else {
                continue;
            };
            adoptions.insert(
                index,
                LoafRegistryAdoption {
                    package: package.name.clone(),
                    version: package.version.clone(),
                    checksum: package.checksum.clone(),
                    index_line_digest: package.index_line_digest(),
                    manifest_digest: package.manifest_digest.clone(),
                    manifest_root: package.manifest_root.clone(),
                    status: package.binding_status(&selection),
                    record: record.clone(),
                },
            );
        }
        Ok(Self { adoptions })
    }

    /// Fill unadopted units from complete proposals written by this capture's own harvest.
    ///
    /// Same-run proposals are transition authority only: every package, source checksum and selection field must
    /// equal the captured unit, and the proposal must convert through the same typed admission boundary as a
    /// registry fact. Existing pinned-registry adoptions always win. The captured source path supplies the immutable
    /// crate owner; proposal directories contain generated outputs, not copies of registry source archives.
    pub fn with_same_run_harvest(
        mut self,
        capture: &OvenLegacyCargoSelectedUnitCapture,
        harvest_root: &Path,
        profile: &str,
    ) -> Result<Self, OvenLegacyCargoError> {
        let compiler = capture.compiler.as_ref().ok_or_else(|| {
            OvenLegacyCargoError::Plan("same-run harvest adoption requires a captured compiler selection".to_string())
        })?;
        let mut proposals = Vec::new();
        for entry in fs::read_dir(harvest_root).map_err(|source| OvenLegacyCargoError::Io {
            path: harvest_root.to_path_buf(),
            source,
        })? {
            let entry = entry.map_err(|source| OvenLegacyCargoError::Io {
                path: harvest_root.to_path_buf(),
                source,
            })?;
            let path = entry.path().join(HARVEST_PROPOSAL_FILE);
            if !path.is_file() {
                continue;
            }
            let bytes = fs::read(&path).map_err(|source| OvenLegacyCargoError::Io {
                path: path.clone(),
                source,
            })?;
            let proposal = serde_json::from_slice::<HarvestProposal>(&bytes).map_err(|error| {
                OvenLegacyCargoError::Plan(format!(
                    "same-run harvest proposal {} is invalid: {error}",
                    path.display()
                ))
            })?;
            proposals.push((path, bytes, proposal));
        }
        for (index, unit) in capture.units.iter().enumerate() {
            if self.adoptions.contains_key(&index) || unit.mode == "run-custom-build" {
                continue;
            }
            let Some(source) = unit.registry_source.as_ref() else {
                continue;
            };
            let matches = proposals
                .iter()
                .filter(|(_, _, proposal)| {
                    let [fact] = proposal.rust.facts.as_slice() else {
                        return false;
                    };
                    proposal.project.name == unit.package
                        && proposal.project.version == unit.package_version
                        && proposal.source.checksum == source.checksum
                        && fact.toolchain == compiler.toolchain
                        && unit.platform.as_deref() == Some(fact.target.as_str())
                        && fact.profile == profile
                        && fact.features == unit.effective_features
                })
                .collect::<Vec<_>>();
            let [(path, bytes, proposal)] = matches.as_slice() else {
                if matches.is_empty() {
                    continue;
                }
                return Err(OvenLegacyCargoError::Plan(format!(
                    "same-run harvest contains {} matching proposals for `{}` {}",
                    matches.len(),
                    unit.package,
                    unit.package_version
                )));
            };
            let record = proposal.admitted_record().map_err(|error| {
                OvenLegacyCargoError::Plan(format!(
                    "same-run harvest proposal {} is not admissible: {error}",
                    path.display()
                ))
            })?;
            let source_root = captured_registry_source_root(unit, &source.root_module)?;
            let manifest_digest = oven_model::digest::digest_bytes(bytes);
            self.adoptions.insert(
                index,
                LoafRegistryAdoption {
                    package: unit.package.clone(),
                    version: unit.package_version.clone(),
                    checksum: source.checksum.clone(),
                    index_line_digest: manifest_digest.clone(),
                    manifest_digest,
                    manifest_root: source_root,
                    status: "harvested".to_string(),
                    record,
                },
            );
        }
        Ok(self)
    }

    /// The adoption governing one captured unit, if any.
    pub fn adoption(&self, unit: usize) -> Option<&LoafRegistryAdoption> {
        self.adoptions.get(&unit)
    }

    /// Whether any unit is adopted.
    pub fn is_empty(&self) -> bool {
        self.adoptions.is_empty()
    }

    /// Every adoption in capture order.
    pub fn adoptions(&self) -> impl Iterator<Item = (usize, &LoafRegistryAdoption)> {
        self.adoptions.iter().map(|(index, adoption)| (*index, adoption))
    }

    /// The `oven.lock` record of every adoption: what governed each registry unit, ordered by package and version.
    ///
    /// Two units of one package version (a host and a target compilation, say) that the same record governs
    /// collapse into one entry, since the lock records the statement, not the unit count.
    pub fn registry_records(&self) -> Vec<RegistryRecord> {
        let mut records = self
            .adoptions
            .values()
            .map(|adoption| RegistryRecord {
                package: adoption.package.clone(),
                version: adoption.version.clone(),
                checksum: adoption.checksum.clone(),
                index_line_digest: adoption.index_line_digest.clone(),
                status: adoption.status.clone(),
            })
            .collect::<Vec<_>>();
        records.sort();
        records.dedup();
        records
    }

    /// Canonical identity of the registry content this authority applied, for generation evidence.
    ///
    /// `None` when nothing was adopted, so an envelope without registry input carries no registry evidence.
    pub fn evidence_digest(&self) -> Option<String> {
        if self.adoptions.is_empty() {
            return None;
        }
        let mut lines = self
            .adoptions
            .values()
            .map(|adoption| {
                format!(
                    "{}@{} {} {} {}\n",
                    adoption.package,
                    adoption.version,
                    adoption.checksum,
                    adoption.index_line_digest,
                    adoption.manifest_digest
                )
            })
            .collect::<Vec<_>>();
        lines.sort();
        lines.dedup();
        Some(oven_model::digest::digest_bytes(lines.concat().as_bytes()))
    }

    /// Require the observed build-script facts of one adopted unit to agree with its declaration.
    ///
    /// `facts` is the observation from the unit's build-script edge, or `None` when the unit has none; a declaration
    /// then must state no `cfg`, environment, or generated input. `has_tool_probes` closes the separate capture
    /// channel that a registry declaration also cannot represent.
    pub fn check_observation(
        adoption: &LoafRegistryAdoption,
        facts: Option<&OvenLegacyCargoBuildScriptFacts>,
        has_tool_probes: bool,
    ) -> Result<(), OvenLegacyCargoError> {
        let refuse = |message: String| {
            OvenLegacyCargoError::Plan(format!(
                "Loaf registry record for `{}` {} disagrees with the observed build script: {message}",
                adoption.package, adoption.version
            ))
        };
        if let Some(facts) = facts {
            validate_declared_environment(adoption, facts).map_err(&refuse)?;
            if !facts.linked_libraries.is_empty() || !facts.linked_paths.is_empty() {
                return Err(refuse(
                    "the declaration cannot carry observed linked libraries or search paths".to_string(),
                ));
            }
        } else if !adoption.record.environment.is_empty() {
            return Err(refuse(
                "declares rustc-env values for a unit with no observed build script".to_string(),
            ));
        }
        if has_tool_probes {
            return Err(refuse("the declaration cannot carry observed tool probes".to_string()));
        }
        let mut observed_cfg = facts.map(|facts| facts.cfgs.clone()).unwrap_or_default();
        observed_cfg.sort();
        observed_cfg.dedup();
        if observed_cfg != adoption.record.cfg {
            return Err(refuse(format!(
                "declared cfg {:?}, observed {:?}",
                adoption.record.cfg, observed_cfg
            )));
        }
        let observed_out = facts
            .and_then(|facts| facts.output.as_ref())
            .map(|output| {
                output
                    .members
                    .iter()
                    .map(|member| (member.path.clone(), member.digest.clone()))
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default();
        let declared_out = adoption
            .record
            .out
            .iter()
            .map(|out| (out.name.clone(), out.digest.clone()))
            .collect::<BTreeMap<_, _>>();
        if observed_out != declared_out {
            return Err(refuse(format!(
                "declared generated inputs {:?}, observed {:?}",
                declared_out.keys().collect::<Vec<_>>(),
                observed_out.keys().collect::<Vec<_>>()
            )));
        }
        Ok(())
    }
}

/// Require every declared compile-time environment value to equal its exact build-script observation.
fn validate_declared_environment(
    adoption: &LoafRegistryAdoption,
    facts: &OvenLegacyCargoBuildScriptFacts,
) -> Result<(), String> {
    if adoption.record.environment.len() != facts.environment.len() {
        return Err("declared and observed rustc-env key sets differ".to_string());
    }
    for declared in &adoption.record.environment {
        let Some(observed) = facts.environment.get(&declared.name) else {
            return Err(format!("declared rustc-env `{}` was not observed", declared.name));
        };
        let matches = match (&declared.literal, &declared.out) {
            (Some(value), None) => value == observed,
            (None, Some(relative)) => {
                let expected = if relative == "." {
                    facts.out_dir.clone()
                } else {
                    facts.out_dir.join(relative)
                };
                expected.to_string_lossy() == observed.as_str()
            }
            _ => false,
        };
        if !matches {
            return Err(format!(
                "declared rustc-env `{}` does not equal the observed value",
                declared.name
            ));
        }
    }
    Ok(())
}

/// Recover the captured package root by stripping the registry record's checked root-module path.
fn captured_registry_source_root(
    unit: &super::OvenLegacyCargoSelectedUnit,
    root_module: &str,
) -> Result<PathBuf, OvenLegacyCargoError> {
    let module = Path::new(root_module);
    let component_count = module.components().count();
    let mut root = unit.source_path.as_path();
    for _ in 0..component_count {
        root = root.parent().ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "captured source {} does not end in registry root module `{root_module}`",
                unit.source_path.display()
            ))
        })?;
    }
    if root.join(module) != unit.source_path {
        return Err(OvenLegacyCargoError::Plan(format!(
            "captured source {} does not match registry root module `{root_module}`",
            unit.source_path.display()
        )));
    }
    Ok(root.to_path_buf())
}
