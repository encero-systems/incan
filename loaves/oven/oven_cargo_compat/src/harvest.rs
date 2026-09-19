//! Harvest: turning one compatibility-publisher observation into registry proposals.
//!
//! The publisher observes what every build script emitted under one exact selection. A harvest reads that
//! observation for each registry unit with a build-script edge and writes a proposal — the bound fact record and the
//! evidence of its observation — in the shape `incan-pub add-fact` admits. Harvest is observation; admitting the
//! proposal is declaration, by a maintainer of the registry. Nothing here decides that an answer is true, and a
//! harvest that ran under ambient state a stable publisher never has (`RUSTC_BOOTSTRAP`) says so in its evidence,
//! so admission can refuse it.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use oven_model::manifest::is_sha256_identity;
use serde::Serialize;

use super::{OvenLegacyCargoError, OvenLegacyCargoSelectedUnit, OvenLegacyCargoSelectedUnitCapture};

/// The registry every crates.io source is described against, spelled as the record spells it.
pub const CRATES_IO_INDEX: &str = "https://github.com/rust-lang/crates.io-index";

/// How a harvest names the way its facts were observed.
pub const HARVEST_METHOD: &str = "compatibility publisher observation";

/// One harvest proposal: the JSON `incan-pub add-fact` reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HarvestProposal {
    pub project: HarvestProject,
    pub source: HarvestSource,
    pub rust: HarvestRust,
    pub evidence: HarvestEvidence,
}

/// `project`: the package version the proposal is about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HarvestProject {
    pub name: String,
    pub version: String,
}

/// `source`: the exact crates.io archive the facts were observed for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HarvestSource {
    pub registry: String,
    pub checksum: String,
}

/// `rust`: exactly one bound fact record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HarvestRust {
    pub facts: Vec<HarvestFact>,
}

/// One bound fact record as the registry renders it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HarvestFact {
    pub toolchain: String,
    pub target: String,
    pub profile: String,
    pub features: Vec<String>,
    pub cfg: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub out: Vec<HarvestOut>,
}

/// One generated input the script wrote: named, placed beside the proposal, bound by digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HarvestOut {
    pub name: String,
    pub path: String,
    pub digest: String,
}

/// `evidence`: what the observation ran under. Recorded verbatim on the `fact` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HarvestEvidence {
    pub method: String,
    /// The compatibility receipt whose capture proposed the record, when it is a `sha256:` identity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt: Option<String>,
    pub rustc_identity: String,
    pub host: String,
    /// Ambient state a stable publisher never has. Admission refuses a proposal that names any.
    pub hazards: Vec<String>,
}

/// One harvested record: the proposal and the generated inputs to place beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarvestedRecord {
    pub proposal: HarvestProposal,
    /// Generated inputs by proposal-relative path, each read from the retained output tree.
    pub out_files: Vec<(String, PathBuf)>,
}

impl HarvestedRecord {
    /// A stable directory name for this proposal: `<name>-<version>-<profile>[-<n>]` is chosen by the writer.
    pub fn stem(&self) -> String {
        format!(
            "{}-{}-{}",
            self.proposal.project.name, self.proposal.project.version, self.proposal.rust.facts[0].profile
        )
    }
}

/// Ambient variables whose presence makes a build script's answers something other than a stable toolchain's.
pub const HARVEST_HAZARD_VARIABLES: &[&str] = &["RUSTC_BOOTSTRAP"];

/// The hazards present in this process's environment, by variable name.
pub fn ambient_harvest_hazards() -> Vec<String> {
    HARVEST_HAZARD_VARIABLES
        .iter()
        .filter(|name| std::env::var_os(name).is_some())
        .map(|name| (*name).to_string())
        .collect()
}

/// Harvest one bound fact record for every registry unit in the capture that has a build-script edge.
///
/// The binding is exactly the one `LoafRegistryAuthority::resolve` would look up: the captured compiler's toolchain
/// and target, the publisher's profile, and the unit's complete effective feature set, sorted. Two units with the
/// same package, version and binding (a host and a target copy) harvest once and must agree; a disagreement is a
/// refusal, because a record cannot state two answers for one binding.
pub fn harvest_registry_build_script_facts(
    capture: &OvenLegacyCargoSelectedUnitCapture,
    profile: &str,
    receipt_identity: &str,
    hazards: Vec<String>,
) -> Result<Vec<HarvestedRecord>, OvenLegacyCargoError> {
    let compiler = capture
        .compiler
        .as_ref()
        .ok_or_else(|| OvenLegacyCargoError::Plan("harvest requires a captured compiler selection".to_string()))?;
    let receipt = {
        let bare = receipt_identity.strip_prefix("sha256:").unwrap_or(receipt_identity);
        let canonical = format!("sha256:{bare}");
        is_sha256_identity(&canonical).then_some(canonical)
    };
    let mut records: BTreeMap<(String, String, String), HarvestedRecord> = BTreeMap::new();
    for unit in &capture.units {
        if unit.mode == "run-custom-build" {
            continue;
        }
        let Some(registry_source) = unit.registry_source.as_ref() else {
            continue;
        };
        let Some(facts) = build_script_facts(capture, unit) else {
            continue;
        };
        let mut features = unit.effective_features.clone();
        features.sort();
        features.dedup();
        let mut cfg = facts.cfgs.clone();
        cfg.sort();
        cfg.dedup();
        let mut out = Vec::new();
        let mut out_files = Vec::new();
        if let Some(output) = facts.output.as_ref() {
            let mut names = BTreeSet::new();
            for member in &output.members {
                if !names.insert(member.path.clone()) {
                    return Err(OvenLegacyCargoError::Plan(format!(
                        "harvest of `{}` {}: generated input `{}` is inventoried twice",
                        unit.package, unit.package_version, member.path
                    )));
                }
                let relative = format!("out/{}", member.path);
                out.push(HarvestOut {
                    name: member.path.clone(),
                    path: relative.clone(),
                    digest: member.digest.clone(),
                });
                out_files.push((relative, facts.out_dir.join(&member.path)));
            }
        }
        let checksum = {
            let bare = registry_source
                .checksum
                .strip_prefix("sha256:")
                .unwrap_or(&registry_source.checksum);
            format!("sha256:{bare}")
        };
        let record = HarvestedRecord {
            proposal: HarvestProposal {
                project: HarvestProject {
                    name: unit.package.clone(),
                    version: unit.package_version.clone(),
                },
                source: HarvestSource {
                    registry: canonical_registry(&registry_source.registry),
                    checksum,
                },
                rust: HarvestRust {
                    facts: vec![HarvestFact {
                        toolchain: compiler.toolchain.clone(),
                        target: compiler.target.clone(),
                        profile: profile.to_string(),
                        features: features.clone(),
                        cfg,
                        out,
                    }],
                },
                evidence: HarvestEvidence {
                    method: HARVEST_METHOD.to_string(),
                    receipt: receipt.clone(),
                    rustc_identity: compiler.rustc_identity.clone(),
                    host: compiler.host.clone(),
                    hazards: hazards.clone(),
                },
            },
            out_files,
        };
        let key = (unit.package.clone(), unit.package_version.clone(), features.join("\n"));
        match records.get(&key) {
            Some(existing) if existing.proposal != record.proposal => {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "harvest of `{}` {}: two captured units under one binding observed different facts (cfg {:?} vs {:?})",
                    unit.package,
                    unit.package_version,
                    existing.proposal.rust.facts[0].cfg,
                    record.proposal.rust.facts[0].cfg
                )));
            }
            Some(_) => {}
            None => {
                records.insert(key, record);
            }
        }
    }
    Ok(records.into_values().collect())
}

/// The observed build-script facts governing `unit`, from its edge to the script unit or from the script unit itself.
fn build_script_facts<'a>(
    capture: &'a OvenLegacyCargoSelectedUnitCapture,
    unit: &'a OvenLegacyCargoSelectedUnit,
) -> Option<&'a super::OvenLegacyCargoBuildScriptFacts> {
    unit.dependencies.iter().find_map(|dependency| {
        let build_unit = capture.units.get(dependency.unit_index)?;
        if build_unit.mode != "run-custom-build" {
            return None;
        }
        dependency.build_script.as_ref().or(build_unit.build_script.as_ref())
    })
}

/// Cargo spells a registry source `registry+<url>`; the record spells the url.
fn canonical_registry(registry: &str) -> String {
    registry.strip_prefix("registry+").unwrap_or(registry).to_string()
}

/// Write every harvested record under `root`: `<stem>/proposal.json` beside its generated inputs.
///
/// Returns the proposal paths written. A stem two records share (one package, two bindings in one profile) gets a
/// numeric suffix in harvest order.
pub fn write_harvest_proposals(root: &Path, records: &[HarvestedRecord]) -> Result<Vec<PathBuf>, OvenLegacyCargoError> {
    let io = |path: &Path, source: std::io::Error| OvenLegacyCargoError::Io {
        path: path.to_path_buf(),
        source,
    };
    fs::create_dir_all(root).map_err(|error| io(root, error))?;
    let mut used = BTreeSet::new();
    let mut written = Vec::new();
    for record in records {
        let mut stem = record.stem();
        let mut suffix = 1;
        while !used.insert(stem.clone()) {
            suffix += 1;
            stem = format!("{}-{suffix}", record.stem());
        }
        let directory = root.join(&stem);
        fs::create_dir_all(&directory).map_err(|error| io(&directory, error))?;
        for (relative, source) in &record.out_files {
            let destination = directory.join(relative);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent).map_err(|error| io(parent, error))?;
            }
            fs::copy(source, &destination).map_err(|error| io(source, error))?;
        }
        let path = directory.join("proposal.json");
        let text = serde_json::to_string_pretty(&record.proposal).map_err(|error| {
            OvenLegacyCargoError::Plan(format!("harvest proposal for `{stem}` is not serializable: {error}"))
        })?;
        fs::write(&path, format!("{text}\n")).map_err(|error| io(&path, error))?;
        written.push(path);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        OvenLegacyCargoBuildScriptFacts, OvenLegacyCargoInspectionSourceMember, OvenLegacyCargoSelectedCompilerContext,
        OvenLegacyCargoSelectedDependency, OvenLegacyCargoSelectedGeneratedOutput,
        OvenLegacyCargoSelectedRegistrySource,
    };
    use oven_rustc::rustc::OvenSelectedRustFacetCfgSnapshot;

    fn cfg_snapshot() -> OvenSelectedRustFacetCfgSnapshot {
        OvenSelectedRustFacetCfgSnapshot {
            flags: vec!["unix".to_string()],
            values: BTreeMap::from([("target_os".to_string(), vec!["macos".to_string()])]),
        }
    }

    fn unit(target_name: &str, mode: &str) -> OvenLegacyCargoSelectedUnit {
        OvenLegacyCargoSelectedUnit {
            package_id: "registry+https://github.com/rust-lang/crates.io-index#serde_core@1.0.228".to_string(),
            package: "serde_core".to_string(),
            package_version: "1.0.228".to_string(),
            package_source: Some("registry+https://github.com/rust-lang/crates.io-index".to_string()),
            target_name: target_name.to_string(),
            target_kinds: vec!["lib".to_string()],
            crate_types: vec!["lib".to_string()],
            source_path: PathBuf::from("/transient/serde_core/src/lib.rs"),
            artifact_paths: Vec::new(),
            root_module: "src/lib.rs".to_string(),
            edition: "2021".to_string(),
            mode: mode.to_string(),
            platform: Some("aarch64-apple-darwin".to_string()),
            target_is_explicit: Some(true),
            cfg: Vec::new(),
            effective_features: Vec::new(),
            dependencies: Vec::new(),
            sysroot_externs: Vec::new(),
            build_script: None,
            registry_source: None,
        }
    }

    fn capture_with_script(
        cfgs: &[&str],
        out_dir: &Path,
        member: Option<(&str, &str)>,
    ) -> OvenLegacyCargoSelectedUnitCapture {
        let mut script = unit("build-script-build", "run-custom-build");
        script.build_script = Some(OvenLegacyCargoBuildScriptFacts {
            cfgs: cfgs.iter().map(|cfg| cfg.to_string()).collect(),
            environment: BTreeMap::from([("CARGO_PKG_X".to_string(), "1".to_string())]),
            linked_libraries: Vec::new(),
            linked_paths: Vec::new(),
            out_dir: out_dir.to_path_buf(),
            output: member.map(|(name, digest)| OvenLegacyCargoSelectedGeneratedOutput {
                relative_root: "out".to_string(),
                digest: "sha256:tree".to_string(),
                members: vec![OvenLegacyCargoInspectionSourceMember {
                    path: name.to_string(),
                    digest: digest.to_string(),
                }],
            }),
        });
        let mut lib = unit("serde_core", "build");
        lib.effective_features = vec![
            "std".to_string(),
            "alloc".to_string(),
            "default".to_string(),
            "result".to_string(),
        ];
        lib.registry_source = Some(OvenLegacyCargoSelectedRegistrySource {
            registry: "registry+https://github.com/rust-lang/crates.io-index".to_string(),
            checksum: "41d385c7d4ca58e59fc732af25c3983b67ac852c1a25000afe1175de458b67ad".to_string(),
            digest: "sha256:src".to_string(),
            root_module: "src/lib.rs".to_string(),
            members: Vec::new(),
        });
        lib.dependencies.push(OvenLegacyCargoSelectedDependency {
            unit_index: 1,
            extern_crate_name: None,
            build_script: None,
        });
        OvenLegacyCargoSelectedUnitCapture {
            roots: vec![0],
            units: vec![lib, script],
            rustc_invocations_observed: true,
            build_script_tool_probes: Vec::new(),
            compiler: Some(OvenLegacyCargoSelectedCompilerContext {
                host: "aarch64-apple-darwin".to_string(),
                target: "aarch64-apple-darwin".to_string(),
                toolchain: "rustc 1.98.0 (88d9e12ae 2026-08-18)".to_string(),
                rustc_identity: "sha256:rustc".to_string(),
                host_cfg: cfg_snapshot(),
                target_cfg: cfg_snapshot(),
            }),
        }
    }

    #[test]
    fn a_registry_unit_with_a_script_edge_harvests_one_bound_record_in_the_proposal_shape()
    -> Result<(), Box<dyn std::error::Error>> {
        let out_dir = tempfile::tempdir()?;
        fs::write(out_dir.path().join("private.rs"), "pub const X: u8 = 1;\n")?;
        let capture = capture_with_script(
            &["b_cfg", "a_cfg", "a_cfg"],
            out_dir.path(),
            Some(("private.rs", "sha256:priv")),
        );
        let records = harvest_registry_build_script_facts(&capture, "release", "abc", Vec::new())?;
        assert_eq!(records.len(), 1);
        let proposal = &records[0].proposal;
        assert_eq!(proposal.project.name, "serde_core");
        assert_eq!(proposal.source.registry, CRATES_IO_INDEX);
        assert_eq!(
            proposal.source.checksum,
            "sha256:41d385c7d4ca58e59fc732af25c3983b67ac852c1a25000afe1175de458b67ad"
        );
        let fact = &proposal.rust.facts[0];
        assert_eq!(fact.features, ["alloc", "default", "result", "std"]);
        assert_eq!(fact.cfg, ["a_cfg", "b_cfg"]);
        assert_eq!(
            fact.out,
            [HarvestOut {
                name: "private.rs".into(),
                path: "out/private.rs".into(),
                digest: "sha256:priv".into()
            }]
        );
        assert_eq!(
            proposal.evidence.receipt, None,
            "a receipt that is not a sha256 identity is not named"
        );
        assert!(proposal.evidence.hazards.is_empty());
        let harvest_root = out_dir.path().join("harvest");
        let written = write_harvest_proposals(&harvest_root, &records)?;
        assert_eq!(written, [harvest_root.join("serde_core-1.0.228-release/proposal.json")]);
        let json: serde_json::Value = serde_json::from_str(&fs::read_to_string(&written[0])?)?;
        assert_eq!(json["rust"]["facts"][0]["out"][0]["path"], "out/private.rs");
        assert_eq!(json["evidence"]["method"], HARVEST_METHOD);
        assert_eq!(
            fs::read_to_string(harvest_root.join("serde_core-1.0.228-release/out/private.rs"))?,
            "pub const X: u8 = 1;\n"
        );
        Ok(())
    }

    #[test]
    fn hazards_and_a_receipt_identity_are_carried_as_evidence() -> Result<(), Box<dyn std::error::Error>> {
        let out_dir = tempfile::tempdir()?;
        let capture = capture_with_script(&[], out_dir.path(), None);
        let receipt = format!("sha256:{}", "a".repeat(64));
        let records =
            harvest_registry_build_script_facts(&capture, "debug", &receipt, vec!["RUSTC_BOOTSTRAP".to_string()])?;
        let proposal = &records[0].proposal;
        assert_eq!(proposal.evidence.receipt.as_deref(), Some(receipt.as_str()));
        assert_eq!(proposal.evidence.hazards, ["RUSTC_BOOTSTRAP"]);
        assert!(proposal.rust.facts[0].out.is_empty());
        assert!(records[0].out_files.is_empty());
        let json = serde_json::to_value(proposal)?;
        assert!(
            json["rust"]["facts"][0].get("out").is_none(),
            "an empty out is omitted, not stated"
        );
        Ok(())
    }

    #[test]
    fn two_units_under_one_binding_must_observe_the_same_facts() -> Result<(), Box<dyn std::error::Error>> {
        let out_dir = tempfile::tempdir()?;
        let mut capture = capture_with_script(&["a_cfg"], out_dir.path(), None);
        let mut host_copy = capture.units[0].clone();
        let mut host_script = capture.units[1].clone();
        host_script
            .build_script
            .as_mut()
            .ok_or("fixture script has facts")?
            .cfgs = vec!["other".to_string()];
        host_copy.dependencies[0].unit_index = 3;
        capture.units.push(host_copy);
        capture.units.push(host_script);
        let error = harvest_registry_build_script_facts(&capture, "release", "r", Vec::new()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("two captured units under one binding observed different facts"),
            "{error}"
        );
        Ok(())
    }
}
