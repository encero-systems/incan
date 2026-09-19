//! Declared build facts a registered Loaf registry supplies for captured units.
//!
//! The compatibility publisher observes what each build script emitted. Where a registry publishes an adoption
//! manifest for the exact captured source and selection, that declaration is the unit's authority under RFC 119:
//! the projection carries the declared facts and nothing a script emitted outside them. While both exist, the
//! declaration and the observation must agree on every fact they both state, so a stale or wrong record is a
//! refusal rather than a quiet substitution.

use std::collections::BTreeMap;

use oven_model::loaf_registry::LoafRegistry;
use oven_model::lock::RegistryRecord;
use oven_model::manifest::{RustFactRecord, RustFactSelection};

use super::{OvenLegacyCargoBuildScriptFacts, OvenLegacyCargoError, OvenLegacyCargoSelectedUnitCapture};

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
            let selection = RustFactSelection {
                toolchain: compiler.toolchain.clone(),
                target: compiler.target.clone(),
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
                    status: package.binding_status(&selection),
                    record: record.clone(),
                },
            );
        }
        Ok(Self { adoptions })
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
        let lines = self
            .adoptions
            .values()
            .map(|adoption| {
                format!(
                    "{}@{} {} {}\n",
                    adoption.package, adoption.version, adoption.checksum, adoption.index_line_digest
                )
            })
            .collect::<String>();
        Some(oven_model::digest::digest_bytes(lines.as_bytes()))
    }

    /// Require the observed build-script facts of one adopted unit to agree with its declaration.
    ///
    /// `facts` is the observation from the unit's build-script edge, or `None` when the unit has none; a declaration
    /// then must state no `cfg` answer and no generated input.
    pub fn check_observation(
        adoption: &LoafRegistryAdoption,
        facts: Option<&OvenLegacyCargoBuildScriptFacts>,
    ) -> Result<(), OvenLegacyCargoError> {
        let refuse = |message: String| {
            OvenLegacyCargoError::Plan(format!(
                "Loaf registry record for `{}` {} disagrees with the observed build script: {message}",
                adoption.package, adoption.version
            ))
        };
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
