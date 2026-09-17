//! Physical selected-unit evidence retained from Cargo only at the explicit compatibility publisher boundary.
//!
//! This capture preserves Cargo facts without assigning Incan dependency intent. A later Oven adapter joins these
//! units to staged source inventories, cfg snapshots, toolchain ownership and the sealed project inspection authority
//! before constructing a selected Rust facet graph.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{CargoBuildScriptExecuted, CargoInvocationOutput, CargoMetadata, CargoUnitGraph, OvenLegacyCargoError};

/// Publisher-only physical facts for one Cargo selected-unit graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenLegacyCargoSelectedUnitCapture {
    /// Cargo unit-graph roots retained as indices into `units`.
    pub roots: Vec<usize>,
    /// Every physical unit selected by Cargo, preserving graph order for dependency indices.
    pub units: Vec<OvenLegacyCargoSelectedUnit>,
}

/// One Cargo-selected physical compilation unit before Oven owner and intent admission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenLegacyCargoSelectedUnit {
    pub package_id: String,
    pub package: String,
    pub package_version: String,
    pub package_source: Option<String>,
    pub target_name: String,
    pub target_kinds: Vec<String>,
    pub crate_types: Vec<String>,
    pub source_path: PathBuf,
    pub edition: String,
    pub mode: String,
    pub platform: Option<String>,
    pub effective_features: Vec<String>,
    pub dependencies: Vec<OvenLegacyCargoSelectedDependency>,
    pub build_script: Option<OvenLegacyCargoBuildScriptFacts>,
}

/// One exact Cargo unit-graph edge and its Rust extern alias when present.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenLegacyCargoSelectedDependency {
    pub unit_index: usize,
    pub extern_crate_name: Option<String>,
}

/// Structured cfg, environment, native-link and output-directory facts emitted by one executed build script.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenLegacyCargoBuildScriptFacts {
    pub cfgs: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub linked_libraries: Vec<String>,
    pub linked_paths: Vec<String>,
    pub out_dir: PathBuf,
}

/// Capture Cargo's physical unit selection and structured build-script results without deriving semantic intent.
pub fn capture_legacy_cargo_selected_units(
    graph: &CargoUnitGraph,
    metadata: &CargoMetadata,
    outputs: &[CargoInvocationOutput],
) -> Result<OvenLegacyCargoSelectedUnitCapture, OvenLegacyCargoError> {
    if graph.version != 1 || graph.roots.is_empty() {
        return Err(OvenLegacyCargoError::Plan(
            "selected-unit capture requires Cargo unit-graph version 1 with at least one root".to_string(),
        ));
    }
    let packages = metadata
        .packages
        .iter()
        .map(|package| (package.id.as_str(), package))
        .collect::<BTreeMap<_, _>>();
    if let Some(root) = graph.roots.iter().find(|root| **root >= graph.units.len()) {
        return Err(OvenLegacyCargoError::Plan(format!(
            "Cargo unit root index {root} is outside its unit graph"
        )));
    }
    let mut build_script_records = BTreeMap::<String, Vec<CargoBuildScriptExecuted>>::new();
    for output in outputs {
        for line in output
            .stdout
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
        {
            let Ok(value) = serde_json::from_slice::<serde_json::Value>(line) else {
                continue;
            };
            if value.get("reason").and_then(serde_json::Value::as_str) != Some("build-script-executed") {
                continue;
            }
            let record = serde_json::from_value::<CargoBuildScriptExecuted>(value).map_err(|error| {
                OvenLegacyCargoError::Plan(format!("invalid Cargo build-script execution record: {error}"))
            })?;
            if record.reason != "build-script-executed" {
                return Err(OvenLegacyCargoError::Plan(
                    "Cargo build-script record has an inconsistent reason".to_string(),
                ));
            }
            build_script_records
                .entry(record.package_id.clone())
                .or_default()
                .push(record);
        }
    }
    if let Some(package_id) = build_script_records.keys().find(|package_id| {
        !graph.units.iter().any(|unit| {
            &unit.pkg_id == *package_id
                && unit.mode == "run-custom-build"
                && unit.target.kind.iter().any(|kind| kind == "custom-build")
        })
    }) {
        return Err(OvenLegacyCargoError::Plan(format!(
            "Cargo emitted build-script facts without a selected run-custom-build unit for `{package_id}`"
        )));
    }
    let mut build_script_units = BTreeMap::new();
    for (index, unit) in graph.units.iter().enumerate().filter(|(_, unit)| {
        unit.mode == "run-custom-build" && unit.target.kind.iter().any(|kind| kind == "custom-build")
    }) {
        if build_script_units.insert(unit.pkg_id.as_str(), index).is_some() {
            return Err(OvenLegacyCargoError::Plan(format!(
                "Cargo selected multiple run-custom-build units for `{}` without per-record unit identity",
                unit.pkg_id
            )));
        }
        if !graph.units.iter().any(|consumer| {
            consumer.pkg_id == unit.pkg_id
                && consumer.mode != "run-custom-build"
                && consumer.dependencies.iter().any(|dependency| dependency.index == index)
        }) {
            return Err(OvenLegacyCargoError::Plan(format!(
                "Cargo run-custom-build unit for `{}` has no same-package consumer edge",
                unit.pkg_id
            )));
        }
        if !build_script_records.contains_key(&unit.pkg_id) {
            return Err(OvenLegacyCargoError::Plan(format!(
                "Cargo selected a run-custom-build unit for `{}` without structured execution facts",
                unit.pkg_id
            )));
        }
    }
    for (package_id, records) in &build_script_records {
        if records.len() != 1 {
            return Err(OvenLegacyCargoError::Plan(format!(
                "Cargo emitted multiple build-script records for `{package_id}` without per-record unit identity"
            )));
        }
    }
    let mut units = Vec::with_capacity(graph.units.len());
    for unit in &graph.units {
        let package = packages.get(unit.pkg_id.as_str()).ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "Cargo unit graph package `{}` is absent from publisher metadata",
                unit.pkg_id
            ))
        })?;
        let mut features = unit.features.clone();
        features.sort();
        features.dedup();
        let build_script = build_script_units
            .get(unit.pkg_id.as_str())
            .filter(|index| **index == units.len())
            .and_then(|_| build_script_records.get(&unit.pkg_id))
            .and_then(|records| records.first())
            .map(|record| {
                let mut cfgs = record.cfgs.clone();
                cfgs.sort();
                cfgs.dedup();
                let mut environment = BTreeMap::new();
                for (name, value) in &record.env {
                    if environment.insert(name.clone(), value.clone()).is_some() {
                        return Err(OvenLegacyCargoError::Plan(format!(
                            "Cargo build-script facts repeat environment key `{name}` for `{}`",
                            unit.pkg_id
                        )));
                    }
                }
                Ok(OvenLegacyCargoBuildScriptFacts {
                    cfgs,
                    environment,
                    linked_libraries: record.linked_libs.clone(),
                    linked_paths: record.linked_paths.clone(),
                    out_dir: record.out_dir.clone(),
                })
            })
            .transpose()?;
        let dependencies = unit
            .dependencies
            .iter()
            .map(|dependency| {
                if dependency.index >= graph.units.len() {
                    return Err(OvenLegacyCargoError::Plan(format!(
                        "Cargo unit dependency index {} is outside its unit graph",
                        dependency.index
                    )));
                }
                Ok(OvenLegacyCargoSelectedDependency {
                    unit_index: dependency.index,
                    extern_crate_name: dependency.extern_crate_name.clone(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        units.push(OvenLegacyCargoSelectedUnit {
            package_id: unit.pkg_id.clone(),
            package: package.name.clone(),
            package_version: package.version.clone(),
            package_source: package.source.clone(),
            target_name: unit.target.name.clone(),
            target_kinds: unit.target.kind.clone(),
            crate_types: unit.target.crate_types.clone(),
            source_path: unit.target.src_path.clone(),
            edition: unit.target.edition.clone(),
            mode: unit.mode.clone(),
            platform: unit.platform.clone(),
            effective_features: features,
            dependencies,
            build_script,
        });
    }
    Ok(OvenLegacyCargoSelectedUnitCapture {
        roots: graph.roots.clone(),
        units,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_preserves_unit_edges_and_structured_build_script_facts() -> Result<(), Box<dyn std::error::Error>> {
        let graph = serde_json::from_value::<CargoUnitGraph>(serde_json::json!({
            "version": 1,
            "roots": [0],
            "units": [
                {
                    "pkg_id": "path+file:///fixture#root@1.0.0",
                    "target": {"kind": ["lib"], "crate_types": ["lib"], "name": "root", "src_path": "/fixture/src/lib.rs", "edition": "2024"},
                    "mode": "build",
                    "platform": "aarch64-apple-darwin",
                    "features": ["z", "a", "a"],
                    "dependencies": [{"index": 1, "extern_crate_name": "dep_alias"}]
                },
                {
                    "pkg_id": "registry+https://example.invalid/index#dep@2.0.0",
                    "target": {"kind": ["lib"], "crate_types": ["lib"], "name": "dep", "src_path": "/registry/dep/src/lib.rs", "edition": "2021"},
                    "mode": "build",
                    "features": [],
                    "dependencies": [{"index": 2, "extern_crate_name": null}]
                },
                {
                    "pkg_id": "registry+https://example.invalid/index#dep@2.0.0",
                    "target": {"kind": ["custom-build"], "crate_types": ["bin"], "name": "build-script-build", "src_path": "/registry/dep/build.rs", "edition": "2021"},
                    "mode": "run-custom-build",
                    "features": [],
                    "dependencies": []
                }
            ]
        }))?;
        let metadata = serde_json::from_value::<CargoMetadata>(serde_json::json!({
            "packages": [
                {"id": "path+file:///fixture#root@1.0.0", "name": "root", "version": "1.0.0", "manifest_path": "/fixture/Cargo.toml"},
                {"id": "registry+https://example.invalid/index#dep@2.0.0", "name": "dep", "version": "2.0.0", "manifest_path": "/registry/dep/Cargo.toml", "source": "registry+https://example.invalid/index"}
            ]
        }))?;
        let output = CargoInvocationOutput {
            stdout: serde_json::to_vec(&serde_json::json!({
                "reason": "build-script-executed",
                "package_id": "registry+https://example.invalid/index#dep@2.0.0",
                "linked_libs": ["static=second", "static=first", "static=second"],
                "linked_paths": ["native=/tmp/z", "native=/tmp/a", "native=/tmp/z"],
                "cfgs": ["dep_cfg"],
                "env": [["DEP_MODE", "sealed"]],
                "out_dir": "/tmp/dep-out"
            }))?,
        };
        let capture = capture_legacy_cargo_selected_units(&graph, &metadata, &[output])?;
        assert_eq!(capture.roots, [0]);
        assert_eq!(capture.units[0].effective_features, ["a", "z"]);
        assert_eq!(
            capture.units[0].dependencies[0].extern_crate_name.as_deref(),
            Some("dep_alias")
        );
        assert!(capture.units[1].build_script.is_none());
        let build_script = capture.units[2]
            .build_script
            .as_ref()
            .ok_or("missing build-script facts")?;
        assert_eq!(build_script.cfgs, ["dep_cfg"]);
        assert_eq!(
            build_script.environment.get("DEP_MODE").map(String::as_str),
            Some("sealed")
        );
        assert_eq!(
            build_script.linked_libraries,
            ["static=second", "static=first", "static=second"]
        );
        assert_eq!(
            build_script.linked_paths,
            ["native=/tmp/z", "native=/tmp/a", "native=/tmp/z"]
        );
        Ok(())
    }

    #[test]
    fn capture_refuses_unselected_build_script_facts() -> Result<(), Box<dyn std::error::Error>> {
        let graph = serde_json::from_value::<CargoUnitGraph>(serde_json::json!({
            "version": 1,
            "roots": [0],
            "units": [{
                "pkg_id": "path+file:///fixture#root@1.0.0",
                "target": {"kind": ["lib"], "crate_types": ["lib"], "name": "root", "src_path": "/fixture/src/lib.rs", "edition": "2024"},
                "mode": "build", "features": [], "dependencies": []
            }]
        }))?;
        let metadata = serde_json::from_value::<CargoMetadata>(serde_json::json!({
            "packages": [{"id": "path+file:///fixture#root@1.0.0", "name": "root", "version": "1.0.0", "manifest_path": "/fixture/Cargo.toml"}]
        }))?;
        let output = CargoInvocationOutput {
            stdout: serde_json::to_vec(&serde_json::json!({
                "reason": "build-script-executed", "package_id": "missing", "out_dir": "/tmp/missing"
            }))?,
        };
        assert!(capture_legacy_cargo_selected_units(&graph, &metadata, &[output]).is_err());
        Ok(())
    }

    #[test]
    fn capture_refuses_invalid_roots_and_ambiguous_build_script_units() -> Result<(), Box<dyn std::error::Error>> {
        let metadata = serde_json::from_value::<CargoMetadata>(serde_json::json!({
            "packages": [{"id": "path+file:///fixture#root@1.0.0", "name": "root", "version": "1.0.0", "manifest_path": "/fixture/Cargo.toml"}]
        }))?;
        let invalid_root = serde_json::from_value::<CargoUnitGraph>(serde_json::json!({
            "version": 1, "roots": [1],
            "units": [{
                "pkg_id": "path+file:///fixture#root@1.0.0",
                "target": {"kind": ["lib"], "crate_types": ["lib"], "name": "root", "src_path": "/fixture/src/lib.rs", "edition": "2024"},
                "mode": "build", "features": [], "dependencies": []
            }]
        }))?;
        assert!(capture_legacy_cargo_selected_units(&invalid_root, &metadata, &[]).is_err());

        let ambiguous = serde_json::from_value::<CargoUnitGraph>(serde_json::json!({
            "version": 1, "roots": [0],
            "units": [
                {
                    "pkg_id": "path+file:///fixture#root@1.0.0",
                    "target": {"kind": ["lib"], "crate_types": ["lib"], "name": "root", "src_path": "/fixture/src/lib.rs", "edition": "2024"},
                    "mode": "build", "features": [],
                    "dependencies": [{"index": 1, "extern_crate_name": null}, {"index": 2, "extern_crate_name": null}]
                },
                {
                    "pkg_id": "path+file:///fixture#root@1.0.0",
                    "target": {"kind": ["custom-build"], "crate_types": ["bin"], "name": "build-host", "src_path": "/fixture/build.rs", "edition": "2024"},
                    "mode": "run-custom-build", "platform": "aarch64-apple-darwin", "features": [], "dependencies": []
                },
                {
                    "pkg_id": "path+file:///fixture#root@1.0.0",
                    "target": {"kind": ["custom-build"], "crate_types": ["bin"], "name": "build-target", "src_path": "/fixture/build.rs", "edition": "2024"},
                    "mode": "run-custom-build", "platform": "wasm32-unknown-unknown", "features": [], "dependencies": []
                }
            ]
        }))?;
        let output = CargoInvocationOutput {
            stdout: serde_json::to_vec(&serde_json::json!({
                "reason": "build-script-executed", "package_id": "path+file:///fixture#root@1.0.0", "out_dir": "/tmp/root-out"
            }))?,
        };
        let error = capture_legacy_cargo_selected_units(&ambiguous, &metadata, &[output])
            .expect_err("multiple physical build-script units must remain ambiguous");
        assert!(error.to_string().contains("multiple run-custom-build units"));
        Ok(())
    }
}
