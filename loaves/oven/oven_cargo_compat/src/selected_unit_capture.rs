//! Physical selected-unit evidence retained from Cargo only at the explicit compatibility publisher boundary.
//!
//! This capture preserves Cargo facts without assigning Incan dependency intent. A later Oven adapter joins these
//! units to staged source inventories, cfg snapshots, toolchain ownership and the sealed project inspection authority
//! before constructing a selected Rust facet graph.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use oven_rustc::rustc::OvenSelectedRustFacetCfgSnapshot;
use oven_rustc::rustc::substitution::RustcUnitRequest;
use serde::{Deserialize, Serialize};

use super::{
    CargoBuildScriptExecuted, CargoCompilerArtifact, CargoInvocationOutput, CargoMetadata, CargoUnitGraph,
    CargoUnitGraphDependency, CargoUnitGraphTarget, CargoUnitGraphUnit, OvenLegacyCargoError,
    OvenLegacyCargoInspectionSource, OvenLegacyCargoInspectionSourceMember, OvenLegacyRustcInvocation,
    OvenRustcSupportingArtifact, canonical_directory, copy_regular_directory_tree, digest_bytes, digest_source_tree,
    materialized_files_from_directory, regular_file_bytes,
};

/// Reconstruct Cargo's exact compiled-unit edges from the stable compiler-artifact stream and observed rustc argv.
pub fn capture_legacy_cargo_selected_units_from_trace(
    metadata: &CargoMetadata,
    outputs: &[CargoInvocationOutput],
    expected_rustc: &Path,
    rustc_host: &str,
) -> Result<OvenLegacyCargoSelectedUnitCapture, OvenLegacyCargoError> {
    let mut artifacts = Vec::new();
    let mut artifact_records = BTreeMap::<Vec<PathBuf>, CargoCompilerArtifact>::new();
    let mut invocations = Vec::new();
    let mut build_scripts = Vec::new();
    let mut build_script_records = BTreeMap::<(String, PathBuf), CargoBuildScriptExecuted>::new();
    for output in outputs {
        for line in output
            .stdout
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
        {
            let Ok(value) = serde_json::from_slice::<serde_json::Value>(line) else {
                continue;
            };
            match value.get("reason").and_then(serde_json::Value::as_str) {
                Some("compiler-artifact") => {
                    let artifact = serde_json::from_value::<CargoCompilerArtifact>(value.clone()).map_err(|error| {
                        OvenLegacyCargoError::Plan(format!("invalid Cargo compiler-artifact record: {error}"))
                    })?;
                    if artifact.filenames.is_empty() {
                        return Err(OvenLegacyCargoError::Plan(
                            "Cargo compiler artifact has no physical filenames".to_string(),
                        ));
                    }
                    match artifact_records.get(&artifact.filenames) {
                        Some(previous) if previous == &artifact => continue,
                        Some(_) => {
                            return Err(OvenLegacyCargoError::Plan(
                                "Cargo reused physical artifact filenames for conflicting records".to_string(),
                            ));
                        }
                        None => {
                            artifact_records.insert(artifact.filenames.clone(), artifact.clone());
                            artifacts.push(artifact);
                        }
                    }
                }
                Some("incan-rustc-invocation") => {
                    let invocation = serde_json::from_value::<OvenLegacyRustcInvocation>(value).map_err(|error| {
                        OvenLegacyCargoError::Plan(format!("invalid stable rustc invocation record: {error}"))
                    })?;
                    if !invocations.contains(&invocation) {
                        invocations.push(invocation);
                    }
                }
                Some("build-script-executed") => {
                    let record = serde_json::from_value::<CargoBuildScriptExecuted>(value).map_err(|error| {
                        OvenLegacyCargoError::Plan(format!("invalid Cargo build-script execution record: {error}"))
                    })?;
                    let key = (record.package_id.clone(), record.out_dir.clone());
                    match build_script_records.get(&key) {
                        Some(previous) if previous == &record => continue,
                        Some(_) => {
                            return Err(OvenLegacyCargoError::Plan(format!(
                                "Cargo emitted conflicting build-script facts for `{}` without exact variant identity",
                                record.package_id
                            )));
                        }
                        None => {
                            build_script_records.insert(key, record.clone());
                            build_scripts.push(record);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    let packages = metadata
        .packages
        .iter()
        .map(|package| (package.id.as_str(), package))
        .collect::<BTreeMap<_, _>>();
    let expected_rustc = std::fs::canonicalize(expected_rustc).map_err(|source| OvenLegacyCargoError::Io {
        path: expected_rustc.to_path_buf(),
        source,
    })?;
    for invocation in &invocations {
        let observed = std::fs::canonicalize(&invocation.rustc).map_err(|source| OvenLegacyCargoError::Io {
            path: PathBuf::from(&invocation.rustc),
            source,
        })?;
        if observed != expected_rustc {
            return Err(OvenLegacyCargoError::Plan(format!(
                "stable rustc invocation used `{}` instead of the verified compiler `{}`",
                observed.display(),
                expected_rustc.display()
            )));
        }
    }
    struct Matched<'a> {
        invocation: &'a OvenLegacyRustcInvocation,
        artifact: &'a CargoCompilerArtifact,
        emitted: Vec<PathBuf>,
        aliases: Vec<PathBuf>,
    }
    let mut matched: Vec<Matched<'_>> = Vec::new();
    let mut used_invocations = vec![false; invocations.len()];
    for artifact in &artifacts {
        let package = packages.get(artifact.package_id.as_str()).ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "compiler artifact names absent package `{}`",
                artifact.package_id
            ))
        })?;
        let expected_manifest = package
            .manifest_path
            .parent()
            .map(|path| path.to_string_lossy().to_string())
            .ok_or_else(|| OvenLegacyCargoError::Plan("Cargo package manifest has no parent".to_string()))?;
        let candidates = invocations.iter().enumerate().filter_map(|(index, invocation)| {
            let identity_matches = invocation.environment.get("CARGO_MANIFEST_DIR") == Some(&expected_manifest)
                && invocation.environment.get("CARGO_PKG_NAME") == Some(&package.name)
                && argument_value(&invocation.arguments, "--crate-name")
                    .is_some_and(|name| name == artifact.target.name.replace('-', "_"))
                && invocation_source_matches(invocation, &artifact.target.src_path)
                && artifact.profile.test == invocation.arguments.iter().any(|argument| argument == "--test")
                && {
                    artifact.profile.test || {
                        let mut artifact_crate_types = artifact.target.crate_types.clone();
                        artifact_crate_types.sort();
                        artifact_crate_types == observed_crate_types(invocation)
                    }
                }
                && {
                    let mut artifact_features = artifact.features.clone();
                    artifact_features.sort();
                    artifact_features.dedup();
                    artifact_features == rustc_feature_cfgs(&invocation.arguments)
                };
            if !identity_matches {
                return None;
            }
            let emitted = invocation_artifact_paths(invocation, artifact, rustc_host)?;
            Some((index, invocation, emitted))
        });
        let candidates = candidates.collect::<Vec<_>>();
        let [(invocation_index, invocation, emitted)] = candidates.as_slice() else {
            let qualifier = if candidates.is_empty() { "no exact" } else { "ambiguous" };
            return Err(OvenLegacyCargoError::Plan(format!(
                "Cargo compiler artifact for `{}` has {qualifier} rustc invocation",
                artifact.target.name
            )));
        };
        if used_invocations[*invocation_index] {
            let Some(existing) = matched
                .iter_mut()
                .find(|item| std::ptr::eq(item.invocation, *invocation))
            else {
                return Err(OvenLegacyCargoError::Plan(
                    "used rustc invocation lost its artifact binding".to_string(),
                ));
            };
            if existing.artifact.package_id != artifact.package_id
                || existing.artifact.target != artifact.target
                || existing.artifact.features != artifact.features
                || existing.artifact.profile != artifact.profile
                || existing.emitted != *emitted
            {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "stable rustc invocation matched conflicting Cargo artifacts for `{}`",
                    artifact.target.name
                )));
            }
            existing.aliases.extend(artifact.filenames.iter().cloned());
            existing.aliases.sort();
            existing.aliases.dedup();
            continue;
        }
        used_invocations[*invocation_index] = true;
        matched.push(Matched {
            invocation,
            artifact,
            emitted: emitted.clone(),
            aliases: artifact.filenames.clone(),
        });
    }
    let mut build_script_tool_probes = Vec::new();
    for (index, invocation) in invocations
        .iter()
        .enumerate()
        .filter(|(index, _)| !used_invocations[*index])
    {
        let Some(probe_digest) = build_script_tool_probe_digest(invocation) else {
            let crate_name = argument_value(&invocation.arguments, "--crate-name").unwrap_or("<absent>");
            let cargo_crate = invocation
                .environment
                .get("CARGO_CRATE_NAME")
                .map(String::as_str)
                .unwrap_or("<absent>");
            return Err(OvenLegacyCargoError::Plan(format!(
                "stable rustc invocation {index} for crate `{crate_name}` (Cargo crate `{cargo_crate}`) has no Cargo artifact or bounded build-script tool-probe identity"
            )));
        };
        let probe_package = packages
            .iter()
            .filter(|(_, package)| {
                invocation.environment.get("CARGO_PKG_NAME") == Some(&package.name)
                    && invocation.environment.get("CARGO_PKG_VERSION") == Some(&package.version)
                    && package.manifest_path.parent().is_some_and(|parent| {
                        invocation
                            .environment
                            .get("CARGO_MANIFEST_DIR")
                            .is_some_and(|manifest| manifest == parent.to_string_lossy().as_ref())
                    })
            })
            .map(|(identity, _)| *identity)
            .collect::<Vec<_>>();
        let [probe_package] = probe_package.as_slice() else {
            return Err(OvenLegacyCargoError::Plan(format!(
                "stable rustc build-script tool probe {index} does not name one exact Cargo metadata package"
            )));
        };
        let out_dir = invocation
            .environment
            .get("OUT_DIR")
            .map(PathBuf::from)
            .ok_or_else(|| {
                OvenLegacyCargoError::Plan(format!("stable rustc build-script tool probe {index} lost OUT_DIR"))
            })?;
        if !build_script_records.contains_key(&((*probe_package).to_string(), out_dir.clone())) {
            return Err(OvenLegacyCargoError::Plan(format!(
                "stable rustc build-script tool probe {index} has no exact Cargo build-script OUT_DIR record"
            )));
        }
        let target_context = invocation.environment.get("TARGET").cloned().ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "stable rustc build-script tool probe {index} has no target domain"
            ))
        })?;
        let rustc_target = argument_value(&invocation.arguments, "--target")
            .unwrap_or(rustc_host)
            .to_string();
        build_script_tool_probes.push(OvenLegacyCargoBuildScriptToolProbe {
            package_id: (*probe_package).to_string(),
            out_dir,
            target_context,
            rustc_target,
            digest: probe_digest,
        });
    }
    build_script_tool_probes.sort();
    build_script_tool_probes.dedup();
    let mut artifact_units = BTreeMap::new();
    for (index, item) in matched.iter().enumerate() {
        for filename in &item.aliases {
            let filename = filename.to_string_lossy().to_string();
            if artifact_units.insert(filename.clone(), index).is_some() {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "Cargo artifact filename `{filename}` belongs to more than one compiled unit"
                )));
            }
        }
        for filename in &item.emitted {
            let filename = filename.to_string_lossy().to_string();
            if let Some(previous) = artifact_units.insert(filename.clone(), index)
                && previous != index
            {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "rustc emitted filename `{filename}` belongs to more than one compiled unit"
                )));
            }
        }
    }
    let mut units = Vec::new();
    let mut roots = Vec::new();
    for (index, item) in matched.iter().enumerate() {
        let invocation = item.invocation;
        let artifact = item.artifact;
        let crate_types = observed_crate_types(invocation);
        let mode = if artifact.profile.test { "test" } else { "build" }.to_string();
        let externs = extern_arguments(&invocation.arguments)?;
        let dependencies = externs
            .paths
            .into_iter()
            .map(|(alias, path)| {
                let child = artifact_units.get(&path).ok_or_else(|| {
                    OvenLegacyCargoError::Plan(format!("rustc extern `{alias}={path}` has no exact Cargo artifact"))
                })?;
                Ok(CargoUnitGraphDependency {
                    index: *child,
                    extern_crate_name: Some(alias),
                })
            })
            .collect::<Result<Vec<_>, OvenLegacyCargoError>>()?;
        let custom_build = artifact.target.kind.iter().any(|kind| kind == "custom-build");
        if invocation.environment.contains_key("CARGO_PRIMARY_PACKAGE") && !custom_build {
            roots.push(index);
        }
        units.push(CargoUnitGraphUnit {
            pkg_id: artifact.package_id.clone(),
            target: CargoUnitGraphTarget {
                kind: artifact.target.kind.clone(),
                crate_types,
                name: artifact.target.name.clone(),
                src_path: artifact.target.src_path.clone(),
                edition: argument_value(&invocation.arguments, "--edition")
                    .unwrap_or("2015")
                    .to_string(),
            },
            mode: if custom_build {
                "run-custom-build".to_string()
            } else {
                mode
            },
            platform: Some(
                argument_value(&invocation.arguments, "--target")
                    .unwrap_or(rustc_host)
                    .to_string(),
            ),
            features: rustc_feature_cfgs(&invocation.arguments),
            dependencies,
        });
    }
    let mut build_script_edges = BTreeMap::<(usize, usize), CargoBuildScriptExecuted>::new();
    for record in &build_scripts {
        let out_dir = record.out_dir.to_string_lossy();
        let mut bound = false;
        for (index, item) in matched.iter().enumerate() {
            if item.artifact.package_id == record.package_id
                && item
                    .invocation
                    .environment
                    .get("OUT_DIR")
                    .is_some_and(|value| value == out_dir.as_ref())
            {
                let custom = units
                    .iter()
                    .enumerate()
                    .filter(|(_, unit)| {
                        unit.pkg_id == record.package_id
                            && unit.target.kind.iter().any(|kind| kind == "custom-build")
                            && unit.features == units[index].features
                            && unit.platform.as_deref() == Some(rustc_host)
                    })
                    .map(|(custom, _)| custom)
                    .collect::<Vec<_>>();
                let [custom] = custom.as_slice() else {
                    return Err(OvenLegacyCargoError::Plan(format!(
                        "build-script facts for `{}` cannot bind consumer {index} to one exact host custom-build unit",
                        record.package_id
                    )));
                };
                units[index].dependencies.push(CargoUnitGraphDependency {
                    index: *custom,
                    extern_crate_name: None,
                });
                if build_script_edges.insert((index, *custom), record.clone()).is_some() {
                    return Err(OvenLegacyCargoError::Plan(format!(
                        "consumer {index} repeats build-script facts for `{}`",
                        record.package_id
                    )));
                }
                bound = true;
            }
        }
        if !bound {
            return Err(OvenLegacyCargoError::Plan(format!(
                "build-script facts for `{}` match no exact consumer OUT_DIR",
                record.package_id
            )));
        }
    }
    let mut capture = capture_legacy_cargo_selected_units_inner(
        &CargoUnitGraph {
            version: 1,
            units,
            roots,
        },
        metadata,
        outputs,
        false,
    )?;
    for (unit, item) in capture.units.iter_mut().zip(&matched) {
        unit.cfg = rustc_non_feature_cfgs(&item.invocation.arguments);
        unit.sysroot_externs = extern_arguments(&item.invocation.arguments)?.sysroot;
        unit.target_is_explicit = Some(argument_value(&item.invocation.arguments, "--target").is_some());
    }
    for ((consumer, build_unit), record) in build_script_edges {
        let dependency = capture
            .units
            .get_mut(consumer)
            .and_then(|unit| {
                unit.dependencies
                    .iter_mut()
                    .find(|dependency| dependency.unit_index == build_unit && dependency.extern_crate_name.is_none())
            })
            .ok_or_else(|| {
                OvenLegacyCargoError::Plan(format!(
                    "build-script edge {consumer}->{build_unit} disappeared during selected-unit capture"
                ))
            })?;
        dependency.build_script = Some(build_script_facts(&record)?);
    }
    for probe in &build_script_tool_probes {
        let matches = capture
            .units
            .iter()
            .filter(|consumer| consumer.platform.as_deref() == Some(&probe.target_context))
            .flat_map(|consumer| {
                consumer.dependencies.iter().filter(|dependency| {
                    dependency
                        .build_script
                        .as_ref()
                        .is_some_and(|facts| facts.out_dir == probe.out_dir)
                        && capture
                            .units
                            .get(dependency.unit_index)
                            .is_some_and(|build_unit| build_unit.package_id == probe.package_id)
                })
            })
            .count();
        if matches != 1 {
            return Err(OvenLegacyCargoError::Plan(format!(
                "build-script tool probe for `{}` in target context `{}` matches {matches} execution edges",
                probe.package_id, probe.target_context
            )));
        }
    }
    capture.rustc_invocations_observed = true;
    capture.build_script_tool_probes = build_script_tool_probes;
    Ok(capture)
}

/// Convert one exact Cargo execution record into edge-scoped inert build-script facts.
fn build_script_facts(
    record: &CargoBuildScriptExecuted,
) -> Result<OvenLegacyCargoBuildScriptFacts, OvenLegacyCargoError> {
    let mut cfgs = record.cfgs.clone();
    cfgs.sort();
    cfgs.dedup();
    let mut environment = BTreeMap::new();
    for (name, value) in &record.env {
        if environment.insert(name.clone(), value.clone()).is_some() {
            return Err(OvenLegacyCargoError::Plan(format!(
                "Cargo build-script facts repeat environment key `{name}` for `{}`",
                record.package_id
            )));
        }
    }
    Ok(OvenLegacyCargoBuildScriptFacts {
        cfgs,
        environment,
        linked_libraries: record.linked_libs.clone(),
        linked_paths: record.linked_paths.clone(),
        out_dir: record.out_dir.clone(),
        output: None,
    })
}

/// Recognize a compiler probe launched by a build script through Cargo's configured wrapper.
///
/// These invocations deliberately have no Cargo artifact: the build script consumes only success and deletes the
/// temporary metadata. The retained digest still binds the exact probe into physical capture evidence.
fn build_script_tool_probe_digest(invocation: &OvenLegacyRustcInvocation) -> Option<String> {
    if invocation.environment.contains_key("CARGO_CRATE_NAME")
        || invocation.environment.contains_key("CARGO_PRIMARY_PACKAGE")
    {
        return None;
    }
    let manifest_dir = invocation.environment.get("CARGO_MANIFEST_DIR").map(Path::new)?;
    let out_root = invocation.environment.get("OUT_DIR").map(Path::new)?;
    invocation.environment.get("CARGO_PKG_NAME")?;
    invocation.environment.get("CARGO_PKG_VERSION")?;
    let emits = comma_separated_argument_values(&invocation.arguments, "--emit");
    if emits.is_empty()
        || emits
            .iter()
            .any(|emit| !matches!(emit.as_str(), "dep-info" | "metadata"))
    {
        return None;
    }
    let out_dir = argument_value(&invocation.arguments, "--out-dir").map(Path::new)?;
    if !lexically_beneath(out_dir, out_root) {
        return None;
    }
    let sources = invocation
        .arguments
        .iter()
        .filter(|argument| !argument.starts_with('-') && argument.ends_with(".rs"))
        .collect::<Vec<_>>();
    let [source] = sources.as_slice() else {
        return None;
    };
    let source = Path::new(source);
    let resolved_source = if source.is_absolute() {
        source.to_path_buf()
    } else {
        Path::new(&invocation.working_directory).join(source)
    };
    if !lexically_beneath(&resolved_source, manifest_dir) {
        return None;
    }
    let canonical_manifest = std::fs::canonicalize(manifest_dir).ok()?;
    let canonical_source = std::fs::canonicalize(&resolved_source).ok()?;
    if !canonical_source.starts_with(&canonical_manifest) || !canonical_source.is_file() {
        return None;
    }
    let source_relative = canonical_source.strip_prefix(&canonical_manifest).ok()?;
    let out_relative = out_dir.strip_prefix(out_root).ok()?;
    let normalized_arguments = invocation
        .arguments
        .iter()
        .map(|argument| {
            if argument == source.to_string_lossy().as_ref() {
                format!("<package>/{}", source_relative.to_string_lossy())
            } else if argument == out_dir.to_string_lossy().as_ref() {
                format!("<out>/{}", out_relative.to_string_lossy())
            } else {
                argument.clone()
            }
        })
        .collect::<Vec<_>>();
    let semantic_environment = invocation
        .environment
        .iter()
        .filter(|(name, _)| !matches!(name.as_str(), "CARGO_MANIFEST_DIR" | "OUT_DIR"))
        .collect::<BTreeMap<_, _>>();
    let source_digest = digest_bytes(&regular_file_bytes(&canonical_source).ok()?);
    let encoded = serde_json::to_vec(&serde_json::json!({
        "arguments": normalized_arguments,
        "environment": semantic_environment,
        "source_digest": source_digest,
    }))
    .ok()?;
    Some(digest_bytes(&encoded))
}

/// Check lexical containment while rejecting parent traversal and mismatched absolute roots.
fn lexically_beneath(path: &Path, root: &Path) -> bool {
    use std::path::Component;

    if path.is_absolute() != root.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        || root
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return false;
    }
    path.starts_with(root)
}

/// Derive and verify every direct rustc output that backs Cargo's reported artifact aliases.
fn invocation_artifact_paths(
    invocation: &OvenLegacyRustcInvocation,
    artifact: &CargoCompilerArtifact,
    rustc_host: &str,
) -> Option<Vec<PathBuf>> {
    let Ok(request) = RustcUnitRequest::parse(&invocation.arguments, |name| {
        invocation.environment.get(name).map(std::ffi::OsString::from)
    }) else {
        return None;
    };
    let emits = comma_separated_argument_values(&invocation.arguments, "--emit");
    let emitted_kind = |kind: &str| {
        emits.is_empty()
            || emits
                .iter()
                .any(|emit| emit.split_once('=').map_or(emit.as_str(), |value| value.0) == kind)
    };
    let target = request.target.as_deref().unwrap_or(rustc_host);
    let mut emitted = Vec::new();
    for crate_type in observed_crate_types(invocation) {
        let stem = format!("{}{}", request.crate_name, request.extra_filename);
        match crate_type.as_str() {
            "lib" | "rlib" => {
                if emitted_kind("link") {
                    emitted.push(request.out_dir.join(format!("lib{stem}.rlib")));
                }
                if emitted_kind("metadata") {
                    emitted.push(request.out_dir.join(format!("lib{stem}.rmeta")));
                }
            }
            "bin" if emitted_kind("link") => emitted.push(request.out_dir.join(format!(
                "{stem}{}",
                if target.contains("windows") { ".exe" } else { "" }
            ))),
            "proc-macro" | "dylib" | "cdylib" if emitted_kind("link") => {
                let (prefix, suffix) = if target.contains("windows") {
                    ("", ".dll")
                } else if target.contains("apple") || target.contains("darwin") {
                    ("lib", ".dylib")
                } else {
                    ("lib", ".so")
                };
                emitted.push(request.out_dir.join(format!("{prefix}{stem}{suffix}")));
            }
            _ => return None,
        }
    }
    emitted.sort();
    emitted.dedup();
    if emitted.is_empty()
        || !emitted.iter().all(|output| regular_files_equal(output, output))
        || !artifact
            .filenames
            .iter()
            .all(|filename| emitted.iter().any(|output| regular_files_equal(output, filename)))
    {
        return None;
    }
    Some(emitted)
}

/// Return the exact physical crate output classes selected by one rustc invocation.
fn observed_crate_types(invocation: &OvenLegacyRustcInvocation) -> Vec<String> {
    if invocation.arguments.iter().any(|argument| argument == "--test") {
        vec!["bin".to_string()]
    } else {
        let mut crate_types = comma_separated_argument_values(&invocation.arguments, "--crate-type");
        crate_types.sort();
        crate_types
    }
}

/// Resolve rustc's source argument against its recorded working directory before matching Cargo's absolute source.
fn invocation_source_matches(invocation: &OvenLegacyRustcInvocation, source: &Path) -> bool {
    let Ok(request) = RustcUnitRequest::parse(&invocation.arguments, |name| {
        invocation.environment.get(name).map(std::ffi::OsString::from)
    }) else {
        return false;
    };
    let working_directory = Path::new(&invocation.working_directory);
    if request.source.is_absolute() {
        request.source == source
    } else {
        working_directory.join(request.source) == source
    }
}

/// Compare two regular artifact files without following symlinks or retaining either file in memory.
fn regular_files_equal(left: &Path, right: &Path) -> bool {
    let Ok(left_metadata) = std::fs::symlink_metadata(left) else {
        return false;
    };
    let Ok(right_metadata) = std::fs::symlink_metadata(right) else {
        return false;
    };
    if !left_metadata.file_type().is_file()
        || !right_metadata.file_type().is_file()
        || left_metadata.len() != right_metadata.len()
    {
        return false;
    }
    let (Ok(mut left), Ok(mut right)) = (File::open(left), File::open(right)) else {
        return false;
    };
    let mut left_buffer = [0u8; 64 * 1024];
    let mut right_buffer = [0u8; 64 * 1024];
    loop {
        let (Ok(left_read), Ok(right_read)) = (left.read(&mut left_buffer), right.read(&mut right_buffer)) else {
            return false;
        };
        if left_read != right_read || left_buffer[..left_read] != right_buffer[..right_read] {
            return false;
        }
        if left_read == 0 {
            return true;
        }
    }
}

/// Read the first paired or equals-form value for one rustc option.
fn argument_value<'a>(arguments: &'a [String], name: &str) -> Option<&'a str> {
    arguments
        .windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].as_str())
        .or_else(|| {
            arguments
                .iter()
                .find_map(|argument| argument.strip_prefix(&format!("{name}=")))
        })
}

/// Read every paired, comma-separated or equals-form value for one repeatable rustc option.
fn argument_values(arguments: &[String], name: &str) -> Vec<String> {
    arguments
        .windows(2)
        .filter(|pair| pair[0] == name)
        .map(|pair| pair[1].as_str())
        .chain(
            arguments
                .iter()
                .filter_map(|argument| argument.strip_prefix(&format!("{name}="))),
        )
        .map(ToString::to_string)
        .collect()
}

/// Read repeatable rustc values and expand the comma-list grammar used by options such as `--crate-type`.
fn comma_separated_argument_values(arguments: &[String], name: &str) -> Vec<String> {
    argument_values(arguments, name)
        .into_iter()
        .flat_map(|value| value.split(',').map(ToString::to_string).collect::<Vec<_>>())
        .collect()
}

/// Exact path-bearing and verified-sysroot externs decoded from one rustc invocation.
struct CapturedExterns {
    paths: Vec<(String, String)>,
    sysroot: Vec<String>,
}

/// Decode every rustc extern, admitting only the compiler's built-in `proc_macro` crate without a path.
fn extern_arguments(arguments: &[String]) -> Result<CapturedExterns, OvenLegacyCargoError> {
    let mut paths = Vec::new();
    let mut sysroot = Vec::new();
    for value in argument_values(arguments, "--extern") {
        let Some((alias, path)) = value.split_once('=') else {
            if value == "proc_macro" {
                sysroot.push(value);
                continue;
            }
            return Err(OvenLegacyCargoError::Plan(format!(
                "rustc extern `{value}` has no exact artifact path or admitted sysroot identity"
            )));
        };
        if alias.is_empty() || path.is_empty() {
            return Err(OvenLegacyCargoError::Plan(format!(
                "rustc extern `{value}` has an empty alias or artifact path"
            )));
        }
        paths.push((alias.to_string(), path.to_string()));
    }
    sysroot.sort();
    sysroot.dedup();
    Ok(CapturedExterns { paths, sysroot })
}

/// Return the sorted unique Cargo feature cfg values passed to one rustc invocation.
fn rustc_feature_cfgs(arguments: &[String]) -> Vec<String> {
    let mut features = argument_values(arguments, "--cfg")
        .into_iter()
        .filter_map(|cfg| {
            cfg.strip_prefix("feature=\"")
                .and_then(|value| value.strip_suffix('"'))
                .map(ToString::to_string)
        })
        .collect::<Vec<_>>();
    features.sort();
    features.dedup();
    features
}

/// Return sorted unique non-feature cfg values passed to one exact rustc invocation.
fn rustc_non_feature_cfgs(arguments: &[String]) -> Vec<String> {
    let mut cfg = argument_values(arguments, "--cfg")
        .into_iter()
        .filter(|value| !(value.starts_with("feature=\"") && value.ends_with('"')))
        .collect::<Vec<_>>();
    cfg.sort();
    cfg.dedup();
    cfg
}

/// Publisher-only physical facts for one Cargo selected-unit graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenLegacyCargoSelectedUnitCapture {
    /// Cargo unit-graph roots retained as indices into `units`.
    pub roots: Vec<usize>,
    /// Every physical unit selected by Cargo, preserving graph order for dependency indices.
    pub units: Vec<OvenLegacyCargoSelectedUnit>,
    /// Whether every unit was joined bijectively to a successful invocation from the stable rustc trace protocol.
    pub rustc_invocations_observed: bool,
    /// Successful non-linking build-script probes bound to their exact package, OUT_DIR and target domain.
    #[serde(default)]
    pub build_script_tool_probes: Vec<OvenLegacyCargoBuildScriptToolProbe>,
    /// Exact compiler selection and cfg observations; absent until the publisher probes its verified compiler.
    pub compiler: Option<OvenLegacyCargoSelectedCompilerContext>,
}

/// One successful transient compiler probe launched by a build script through Cargo's configured wrapper.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenLegacyCargoBuildScriptToolProbe {
    pub package_id: String,
    pub out_dir: PathBuf,
    pub target_context: String,
    pub rustc_target: String,
    pub digest: String,
}

/// Physical compiler facts captured once for the same host/target selection as the Cargo unit graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenLegacyCargoSelectedCompilerContext {
    pub host: String,
    pub target: String,
    pub toolchain: String,
    pub rustc_identity: String,
    pub host_cfg: OvenSelectedRustFacetCfgSnapshot,
    pub target_cfg: OvenSelectedRustFacetCfgSnapshot,
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
    /// Package-root-relative crate root derived from Cargo metadata and the selected target record.
    pub root_module: String,
    pub edition: String,
    pub mode: String,
    pub platform: Option<String>,
    /// Whether the exact matched rustc invocation carried `--target`.
    ///
    /// `None` is an untraced generic Cargo graph and cannot authorize host/target classification.
    #[serde(default)]
    pub target_is_explicit: Option<bool>,
    /// Exact non-feature rustc cfg arguments; Cargo features remain separately named by `effective_features`.
    pub cfg: Vec<String>,
    pub effective_features: Vec<String>,
    pub dependencies: Vec<OvenLegacyCargoSelectedDependency>,
    /// Bare compiler/sysroot externs admitted from the verified rustc invocation.
    #[serde(default)]
    pub sysroot_externs: Vec<String>,
    pub build_script: Option<OvenLegacyCargoBuildScriptFacts>,
    /// Exact staged registry source evidence, when this is a registry-backed unit.
    pub registry_source: Option<OvenLegacyCargoSelectedRegistrySource>,
}

/// Registry source evidence joined by exact Cargo package coordinates before the transient publisher is released.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenLegacyCargoSelectedRegistrySource {
    pub registry: String,
    pub checksum: String,
    pub digest: String,
    pub root_module: String,
    pub members: Vec<OvenLegacyCargoInspectionSourceMember>,
}

/// One exact Cargo unit-graph edge and its Rust extern alias when present.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenLegacyCargoSelectedDependency {
    pub unit_index: usize,
    pub extern_crate_name: Option<String>,
    /// Exact execution facts when this edge consumes one run-custom-build unit.
    #[serde(default)]
    pub build_script: Option<OvenLegacyCargoBuildScriptFacts>,
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
    /// Retained output tree; `None` means the publisher has not yet copied and inventoried the declared directory.
    pub output: Option<OvenLegacyCargoSelectedGeneratedOutput>,
}

/// Exact retained generated-output tree for one executed compatibility build script.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenLegacyCargoSelectedGeneratedOutput {
    pub relative_root: String,
    pub digest: String,
    pub members: Vec<OvenLegacyCargoInspectionSourceMember>,
}

/// Capture Cargo's physical unit selection and structured build-script results without deriving semantic intent.
pub fn capture_legacy_cargo_selected_units(
    graph: &CargoUnitGraph,
    metadata: &CargoMetadata,
    outputs: &[CargoInvocationOutput],
) -> Result<OvenLegacyCargoSelectedUnitCapture, OvenLegacyCargoError> {
    capture_legacy_cargo_selected_units_inner(graph, metadata, outputs, true)
}

/// Capture units while allowing the traced publisher to attach variant-specific build-script facts to edges later.
fn capture_legacy_cargo_selected_units_inner(
    graph: &CargoUnitGraph,
    metadata: &CargoMetadata,
    outputs: &[CargoInvocationOutput],
    attach_package_build_script_facts: bool,
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
            let records = build_script_records.entry(record.package_id.clone()).or_default();
            if !records.contains(&record) {
                records.push(record);
            }
        }
    }
    if attach_package_build_script_facts
        && let Some(package_id) = build_script_records.keys().find(|package_id| {
            !graph.units.iter().any(|unit| {
                &unit.pkg_id == *package_id
                    && unit.mode == "run-custom-build"
                    && unit.target.kind.iter().any(|kind| kind == "custom-build")
            })
        })
    {
        return Err(OvenLegacyCargoError::Plan(format!(
            "Cargo emitted build-script facts without a selected run-custom-build unit for `{package_id}`"
        )));
    }
    let mut build_script_units = BTreeMap::new();
    for (index, unit) in graph.units.iter().enumerate().filter(|(_, unit)| {
        unit.mode == "run-custom-build" && unit.target.kind.iter().any(|kind| kind == "custom-build")
    }) {
        if !attach_package_build_script_facts {
            continue;
        }
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
        if !attach_package_build_script_facts {
            break;
        }
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
        let package_root = package.manifest_path.parent().ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "Cargo metadata manifest for `{}` has no package root",
                unit.pkg_id
            ))
        })?;
        let root_module = unit
            .target
            .src_path
            .strip_prefix(package_root)
            .map_err(|_| {
                OvenLegacyCargoError::Plan(format!(
                    "Cargo unit `{}` source root escaped its metadata package root",
                    unit.pkg_id
                ))
            })?
            .to_string_lossy()
            .replace('\\', "/");
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
                    output: None,
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
                    build_script: None,
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
            root_module,
            edition: unit.target.edition.clone(),
            mode: unit.mode.clone(),
            platform: unit.platform.clone(),
            target_is_explicit: None,
            cfg: Vec::new(),
            effective_features: features,
            dependencies,
            sysroot_externs: Vec::new(),
            build_script,
            registry_source: None,
        });
    }
    Ok(OvenLegacyCargoSelectedUnitCapture {
        roots: graph.roots.clone(),
        units,
        rustc_invocations_observed: false,
        build_script_tool_probes: Vec::new(),
        compiler: None,
    })
}

/// Copy every declared build-script output directory into publisher staging and retain its exact member inventory.
pub fn retain_legacy_cargo_selected_generated_outputs(
    capture: &mut OvenLegacyCargoSelectedUnitCapture,
    staging: &Path,
) -> Result<Vec<OvenRustcSupportingArtifact>, OvenLegacyCargoError> {
    let mut artifacts = BTreeMap::new();
    for unit in &mut capture.units {
        if let Some(build_script) = unit.build_script.as_mut() {
            retain_generated_output(build_script, &unit.package, staging, &mut artifacts)?;
        }
        for dependency in &mut unit.dependencies {
            if let Some(build_script) = dependency.build_script.as_mut() {
                retain_generated_output(build_script, &unit.package, staging, &mut artifacts)?;
            }
        }
    }
    Ok(artifacts
        .into_iter()
        .map(|(relative_path, digest)| OvenRustcSupportingArtifact { relative_path, digest })
        .collect())
}

/// Retain one exact edge- or unit-scoped build-script output tree.
fn retain_generated_output(
    build_script: &mut OvenLegacyCargoBuildScriptFacts,
    package: &str,
    staging: &Path,
    artifacts: &mut BTreeMap<String, String>,
) -> Result<(), OvenLegacyCargoError> {
    let source = canonical_directory(&build_script.out_dir, "Cargo build-script OUT_DIR")?;
    let source_digest = digest_source_tree(&source).map_err(|error| {
        OvenLegacyCargoError::Plan(format!(
            "could not digest Cargo build-script OUT_DIR for `{}`: {error}",
            package
        ))
    })?;
    let identity = source_digest.strip_prefix("sha256:").unwrap_or(&source_digest);
    let relative_root = format!("generated-outputs/{identity}");
    let destination = staging.join(&relative_root);
    if !destination.exists() {
        copy_regular_directory_tree(&source, &destination, "Cargo build-script OUT_DIR")?;
    }
    let digest = digest_source_tree(&destination).map_err(|error| {
        OvenLegacyCargoError::Plan(format!(
            "could not digest retained Cargo build-script OUT_DIR for `{}`: {error}",
            package
        ))
    })?;
    if digest != source_digest {
        return Err(OvenLegacyCargoError::Plan(format!(
            "Cargo build-script OUT_DIR for `{}` changed while it was retained",
            package
        )));
    }
    let files = materialized_files_from_directory(&destination, &relative_root, "Cargo build-script OUT_DIR")?;
    let mut members = Vec::with_capacity(files.len());
    for file in files {
        let path = file
            .relative_path
            .strip_prefix(&format!("{relative_root}/"))
            .ok_or_else(|| OvenLegacyCargoError::Plan("generated output lost its retained root".to_string()))?
            .to_string();
        let file_digest = digest_bytes(&regular_file_bytes(&file.source_path)?);
        members.push(OvenLegacyCargoInspectionSourceMember {
            path,
            digest: file_digest.clone(),
        });
        if let Some(previous) = artifacts.insert(file.relative_path.clone(), file_digest.clone())
            && previous != file_digest
        {
            return Err(OvenLegacyCargoError::Plan(
                "generated output path has conflicting retained bytes".to_string(),
            ));
        }
    }
    build_script.output = Some(OvenLegacyCargoSelectedGeneratedOutput {
        relative_root,
        digest,
        members,
    });
    Ok(())
}

/// Join registry-backed selected units to the publisher's exact staged source catalogs.
///
/// Path and generated units deliberately remain unbound for the release publisher to join to its separately sealed
/// owner. A registry unit must match exactly one catalog by package, version and Cargo source identity.
pub fn bind_legacy_cargo_selected_registry_sources(
    capture: &mut OvenLegacyCargoSelectedUnitCapture,
    sources: &[OvenLegacyCargoInspectionSource],
) -> Result<(), OvenLegacyCargoError> {
    for unit in &mut capture.units {
        let Some(registry) = unit
            .package_source
            .as_deref()
            .filter(|source| source.starts_with("registry+"))
        else {
            continue;
        };
        let matches = sources
            .iter()
            .filter(|source| {
                source.package == unit.package && source.version == unit.package_version && source.registry == registry
            })
            .collect::<Vec<_>>();
        let [source] = matches.as_slice() else {
            return Err(OvenLegacyCargoError::Plan(format!(
                "selected registry unit `{}` {} from `{registry}` matched {} staged source catalogs",
                unit.package,
                unit.package_version,
                matches.len()
            )));
        };
        let root_module = unit.root_module.clone();
        if root_module.is_empty() || !source.members.iter().any(|member| member.path == root_module) {
            return Err(OvenLegacyCargoError::Plan(format!(
                "selected registry unit `{}` root module is absent from its staged source catalog",
                unit.package
            )));
        }
        unit.registry_source = Some(OvenLegacyCargoSelectedRegistrySource {
            registry: source.registry.clone(),
            checksum: source.checksum.clone(),
            digest: source.source_digest.clone(),
            root_module,
            members: source.members.clone(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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
        let mut capture = capture_legacy_cargo_selected_units(&graph, &metadata, &[output])?;
        let mut missing_source = capture.clone();
        assert!(bind_legacy_cargo_selected_registry_sources(&mut missing_source, &[]).is_err());
        bind_legacy_cargo_selected_registry_sources(
            &mut capture,
            &[OvenLegacyCargoInspectionSource {
                package: "dep".to_string(),
                version: "2.0.0".to_string(),
                registry: "registry+https://example.invalid/index".to_string(),
                checksum: "sealed-checksum".to_string(),
                features: Vec::new(),
                source_root: PathBuf::from("/staged/dep"),
                source_digest: "sha256:sealed-source".to_string(),
                members: vec![
                    OvenLegacyCargoInspectionSourceMember {
                        path: "Cargo.toml".to_string(),
                        digest: "sha256:manifest".to_string(),
                    },
                    OvenLegacyCargoInspectionSourceMember {
                        path: "build.rs".to_string(),
                        digest: "sha256:build".to_string(),
                    },
                    OvenLegacyCargoInspectionSourceMember {
                        path: "src/lib.rs".to_string(),
                        digest: "sha256:lib".to_string(),
                    },
                ],
            }],
        )?;
        assert_eq!(capture.roots, [0]);
        assert_eq!(capture.units[0].effective_features, ["a", "z"]);
        assert_eq!(
            capture.units[0].dependencies[0].extern_crate_name.as_deref(),
            Some("dep_alias")
        );
        assert!(capture.units[1].build_script.is_none());
        assert_eq!(
            capture.units[1]
                .registry_source
                .as_ref()
                .map(|source| source.root_module.as_str()),
            Some("src/lib.rs")
        );
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

    #[test]
    fn retained_generated_output_proves_exact_copied_members() -> Result<(), Box<dyn std::error::Error>> {
        let scratch = tempfile::tempdir()?;
        let output = scratch.path().join("out");
        fs::create_dir_all(output.join("nested"))?;
        fs::write(output.join("generated.rs"), b"pub const GENERATED: bool = true;\n")?;
        fs::write(output.join("nested/data.bin"), b"sealed")?;
        let mut capture = OvenLegacyCargoSelectedUnitCapture {
            roots: vec![0],
            units: vec![OvenLegacyCargoSelectedUnit {
                package_id: "fixture 1.0.0".to_string(),
                package: "fixture".to_string(),
                package_version: "1.0.0".to_string(),
                package_source: None,
                target_name: "build-script-build".to_string(),
                target_kinds: vec!["custom-build".to_string()],
                crate_types: vec!["bin".to_string()],
                source_path: PathBuf::from("/fixture/build.rs"),
                root_module: "build.rs".to_string(),
                edition: "2024".to_string(),
                mode: "run-custom-build".to_string(),
                platform: None,
                target_is_explicit: None,
                cfg: Vec::new(),
                effective_features: Vec::new(),
                dependencies: Vec::new(),
                sysroot_externs: Vec::new(),
                build_script: Some(OvenLegacyCargoBuildScriptFacts {
                    cfgs: Vec::new(),
                    environment: BTreeMap::new(),
                    linked_libraries: Vec::new(),
                    linked_paths: Vec::new(),
                    out_dir: output,
                    output: None,
                }),
                registry_source: None,
            }],
            rustc_invocations_observed: false,
            build_script_tool_probes: Vec::new(),
            compiler: None,
        };
        let staging = scratch.path().join("staging");
        fs::create_dir(&staging)?;
        let artifacts = retain_legacy_cargo_selected_generated_outputs(&mut capture, &staging)?;
        let retained = capture.units[0]
            .build_script
            .as_ref()
            .and_then(|facts| facts.output.as_ref())
            .ok_or("missing retained output")?;
        assert_eq!(
            retained
                .members
                .iter()
                .map(|member| member.path.as_str())
                .collect::<Vec<_>>(),
            ["generated.rs", "nested/data.bin"]
        );
        assert_eq!(artifacts.len(), 2);
        assert!(staging.join(&retained.relative_root).join("generated.rs").is_file());
        Ok(())
    }

    #[test]
    fn stable_trace_binds_exact_extern_path_to_compiled_child() -> Result<(), Box<dyn std::error::Error>> {
        let scratch = tempfile::tempdir()?;
        let rustc = scratch.path().join("rustc");
        fs::write(&rustc, b"verified compiler fixture")?;
        let rustc = rustc.to_string_lossy().to_string();
        let target = scratch.path().join("target");
        fs::create_dir(&target)?;
        let dep_artifact = target.join("libdep-sealed.rlib");
        let root_artifact = target.join("root");
        fs::write(&dep_artifact, b"dep")?;
        fs::write(&root_artifact, b"root")?;
        let metadata = serde_json::from_value::<CargoMetadata>(serde_json::json!({
            "packages": [
                {"id": "root 1.0.0", "name": "root", "version": "1.0.0", "manifest_path": "/fixture/root/Cargo.toml"},
                {"id": "dep 2.0.0", "name": "dep", "version": "2.0.0", "manifest_path": "/fixture/dep/Cargo.toml"}
            ]
        }))?;
        let records = [
            serde_json::json!({
                "reason": "compiler-artifact", "package_id": "dep 2.0.0",
                "target": {"name": "dep", "kind": ["lib"], "crate_types": ["lib"], "src_path": "/fixture/dep/src/lib.rs"},
                "features": [], "filenames": [dep_artifact.clone()], "profile": {"test": false}
            }),
            serde_json::json!({
                "reason": "compiler-artifact", "package_id": "dep 2.0.0", "fresh": true,
                "target": {"name": "dep", "kind": ["lib"], "crate_types": ["lib"], "src_path": "/fixture/dep/src/lib.rs"},
                "features": [], "filenames": [dep_artifact.clone()], "profile": {"test": false}
            }),
            serde_json::json!({
                "reason": "incan-rustc-invocation", "rustc": rustc.clone(),
                "arguments": ["--crate-name", "dep", "--crate-type", "lib", "--edition", "2021", "--cfg", "target_has_atomic=\"ptr\"", "-C", "extra-filename=-sealed", "--emit", "link", "--out-dir", target.to_string_lossy(), "/fixture/dep/src/lib.rs"],
                "environment": {"CARGO_MANIFEST_DIR": "/fixture/dep", "CARGO_PKG_NAME": "dep", "CARGO_PKG_VERSION": "2.0.0"}
            }),
            serde_json::json!({
                "reason": "compiler-artifact", "package_id": "root 1.0.0",
                "target": {"name": "root", "kind": ["bin"], "crate_types": ["bin"], "src_path": "/fixture/root/src/main.rs"},
                "features": [], "filenames": [root_artifact.clone()], "profile": {"test": false}
            }),
            serde_json::json!({
                "reason": "incan-rustc-invocation", "rustc": rustc.clone(),
                "arguments": ["--crate-name", "root", "--crate-type", "bin", "--edition", "2024", "-C", "extra-filename=", "--emit", "link", "--out-dir", target.to_string_lossy(), "--extern", format!("dep={}", dep_artifact.display()), "/fixture/root/src/main.rs"],
                "environment": {"CARGO_MANIFEST_DIR": "/fixture/root", "CARGO_PKG_NAME": "root", "CARGO_PKG_VERSION": "1.0.0", "CARGO_PRIMARY_PACKAGE": "1"}
            }),
        ];
        let mut stdout = Vec::new();
        for record in records {
            stdout.extend_from_slice(&serde_json::to_vec(&record)?);
            stdout.push(b'\n');
        }
        let capture = capture_legacy_cargo_selected_units_from_trace(
            &metadata,
            &[CargoInvocationOutput { stdout }],
            Path::new(&rustc),
            "fixture-host",
        )?;
        assert_eq!(capture.roots, [1]);
        assert!(capture.rustc_invocations_observed);
        assert_eq!(capture.units[0].cfg, ["target_has_atomic=\"ptr\""]);
        assert_eq!(capture.units[1].dependencies[0].unit_index, 0);
        assert_eq!(
            capture.units[1].dependencies[0].extern_crate_name.as_deref(),
            Some("dep")
        );
        Ok(())
    }

    #[test]
    fn stable_trace_distinguishes_host_and_target_variants_by_output() -> Result<(), Box<dyn std::error::Error>> {
        let scratch = tempfile::tempdir()?;
        let rustc = scratch.path().join("rustc");
        fs::write(&rustc, b"verified compiler fixture")?;
        let rustc_name = rustc.to_string_lossy().to_string();
        let host_dir = scratch.path().join("host");
        let wasm_dir = scratch.path().join("wasm");
        fs::create_dir(&host_dir)?;
        fs::create_dir(&wasm_dir)?;
        let host_artifact = host_dir.join("libshared.rlib");
        let wasm_artifact = wasm_dir.join("libshared.rlib");
        fs::write(&host_artifact, b"host")?;
        fs::write(&wasm_artifact, b"wasm")?;
        let metadata = serde_json::from_value::<CargoMetadata>(serde_json::json!({
            "packages": [{
                "id": "shared 1.0.0", "name": "shared", "version": "1.0.0",
                "manifest_path": "/fixture/shared/Cargo.toml"
            }]
        }))?;
        let records = [
            serde_json::json!({
                "reason": "compiler-artifact", "package_id": "shared 1.0.0",
                "target": {"name": "shared", "kind": ["lib"], "crate_types": ["lib"], "src_path": "/fixture/shared/src/lib.rs"},
                "features": [], "filenames": [host_artifact.clone()], "profile": {"test": false}
            }),
            serde_json::json!({
                "reason": "incan-rustc-invocation", "rustc": rustc_name.clone(),
                "arguments": ["--crate-name", "shared", "--crate-type", "lib", "-C", "extra-filename=", "--emit", "link", "--out-dir", host_dir.to_string_lossy(), "/fixture/shared/src/lib.rs"],
                "environment": {"CARGO_MANIFEST_DIR": "/fixture/shared", "CARGO_PKG_NAME": "shared", "CARGO_PKG_VERSION": "1.0.0", "CARGO_PRIMARY_PACKAGE": "1"}
            }),
            serde_json::json!({
                "reason": "compiler-artifact", "package_id": "shared 1.0.0",
                "target": {"name": "shared", "kind": ["lib"], "crate_types": ["lib"], "src_path": "/fixture/shared/src/lib.rs"},
                "features": [], "filenames": [wasm_artifact.clone()], "profile": {"test": false}
            }),
            serde_json::json!({
                "reason": "incan-rustc-invocation", "rustc": rustc_name.clone(),
                "arguments": ["--crate-name", "shared", "--crate-type", "lib", "--target", "wasm32-unknown-unknown", "-C", "extra-filename=", "--emit", "link", "--out-dir", wasm_dir.to_string_lossy(), "/fixture/shared/src/lib.rs"],
                "environment": {"CARGO_MANIFEST_DIR": "/fixture/shared", "CARGO_PKG_NAME": "shared", "CARGO_PKG_VERSION": "1.0.0", "CARGO_PRIMARY_PACKAGE": "1"}
            }),
        ];
        let mut stdout = Vec::new();
        for record in records {
            stdout.extend_from_slice(&serde_json::to_vec(&record)?);
            stdout.push(b'\n');
        }
        let capture = capture_legacy_cargo_selected_units_from_trace(
            &metadata,
            &[CargoInvocationOutput { stdout }],
            &rustc,
            "fixture-host",
        )?;
        assert_eq!(capture.roots, [0, 1]);
        assert_eq!(capture.units[0].platform.as_deref(), Some("fixture-host"));
        assert_eq!(capture.units[1].platform.as_deref(), Some("wasm32-unknown-unknown"));
        assert_eq!(capture.units[0].target_is_explicit, Some(false));
        assert_eq!(capture.units[1].target_is_explicit, Some(true));
        Ok(())
    }

    #[test]
    fn stable_trace_refuses_externs_without_exact_paths() {
        let arguments = vec!["--extern".to_string(), "dependency".to_string()];
        assert!(matches!(
            extern_arguments(&arguments),
            Err(error) if error.to_string().contains("no exact artifact path")
        ));

        let comma_path = vec![
            "--extern".to_string(),
            "dependency=/path,with-comma/libdependency.rlib".to_string(),
        ];
        assert_eq!(
            extern_arguments(&comma_path).ok().map(|externs| externs.paths),
            Some(vec![(
                "dependency".to_string(),
                "/path,with-comma/libdependency.rlib".to_string()
            )])
        );
        let sysroot = vec!["--extern".to_string(), "proc_macro".to_string()];
        let captured = extern_arguments(&sysroot).expect("verified proc_macro sysroot extern should be retained");
        assert!(captured.paths.is_empty());
        assert_eq!(captured.sysroot, ["proc_macro"]);
    }

    #[test]
    fn stable_trace_matches_relative_source_and_content_equal_cargo_alias() -> Result<(), Box<dyn std::error::Error>> {
        let scratch = tempfile::tempdir()?;
        fs::write(scratch.path().join("build.rs"), b"fn main() {}\n")?;
        let out_dir = scratch.path().join("target/debug/build/probe-sealed");
        fs::create_dir_all(&out_dir)?;
        let emitted = out_dir.join("build_script_build-sealed");
        let alias = out_dir.join("build-script-build");
        fs::write(&emitted, b"exact executable bytes")?;
        fs::write(&alias, b"exact executable bytes")?;
        let invocation = OvenLegacyRustcInvocation {
            reason: "incan-rustc-invocation".to_string(),
            rustc: "/verified/rustc".to_string(),
            working_directory: scratch.path().to_string_lossy().to_string(),
            arguments: vec![
                "--crate-name".to_string(),
                "build_script_build".to_string(),
                "--crate-type".to_string(),
                "bin".to_string(),
                "-C".to_string(),
                "extra-filename=-sealed".to_string(),
                "--out-dir".to_string(),
                out_dir.to_string_lossy().to_string(),
                "build.rs".to_string(),
            ],
            environment: BTreeMap::from([
                ("CARGO_PKG_NAME".to_string(), "probe".to_string()),
                ("CARGO_PKG_VERSION".to_string(), "1.0.0".to_string()),
            ]),
        };
        let artifact = serde_json::from_value::<CargoCompilerArtifact>(serde_json::json!({
            "reason": "compiler-artifact", "package_id": "probe 1.0.0",
            "target": {"name": "build-script-build", "kind": ["custom-build"], "crate_types": ["bin"], "src_path": scratch.path().join("build.rs")},
            "features": [], "filenames": [alias], "profile": {"test": false}
        }))?;
        assert!(invocation_source_matches(&invocation, &artifact.target.src_path));
        assert!(invocation_artifact_paths(&invocation, &artifact, "fixture-host").is_some());
        fs::write(&artifact.filenames[0], b"tampered alias bytes")?;
        assert!(invocation_artifact_paths(&invocation, &artifact, "fixture-host").is_none());
        Ok(())
    }

    #[test]
    fn stable_trace_classifies_only_bounded_non_linking_build_script_probes() -> Result<(), Box<dyn std::error::Error>>
    {
        let scratch = tempfile::tempdir()?;
        let source = scratch.path().join("src/probe/span.rs");
        let out_root = scratch.path().join("target/build/probe-package/out");
        fs::create_dir_all(source.parent().ok_or("probe source has no parent")?)?;
        fs::create_dir_all(out_root.join("probe"))?;
        fs::write(&source, b"pub fn probe() {}\n")?;
        let invocation = OvenLegacyRustcInvocation {
            reason: "incan-rustc-invocation".to_string(),
            rustc: "/verified/rustc".to_string(),
            working_directory: scratch.path().to_string_lossy().to_string(),
            arguments: vec![
                "--cfg=build_script_probe".to_string(),
                "--crate-name=probe_package".to_string(),
                "--crate-type=lib".to_string(),
                "--emit=dep-info,metadata".to_string(),
                "--out-dir".to_string(),
                out_root.join("probe").to_string_lossy().to_string(),
                "src/probe/span.rs".to_string(),
            ],
            environment: BTreeMap::from([
                (
                    "CARGO_MANIFEST_DIR".to_string(),
                    scratch.path().to_string_lossy().to_string(),
                ),
                ("CARGO_PKG_NAME".to_string(), "probe-package".to_string()),
                ("CARGO_PKG_VERSION".to_string(), "1.0.0".to_string()),
                ("OUT_DIR".to_string(), out_root.to_string_lossy().to_string()),
            ]),
        };
        assert!(build_script_tool_probe_digest(&invocation).is_some());

        let mut linking = invocation.clone();
        linking.arguments[3] = "--emit=link,metadata".to_string();
        assert!(build_script_tool_probe_digest(&linking).is_none());
        let mut cargo_unit = invocation.clone();
        cargo_unit
            .environment
            .insert("CARGO_CRATE_NAME".to_string(), "probe_package".to_string());
        assert!(build_script_tool_probe_digest(&cargo_unit).is_none());
        let mut escaped = invocation;
        escaped.arguments[5] = scratch.path().join("outside").to_string_lossy().to_string();
        assert!(build_script_tool_probe_digest(&escaped).is_none());
        Ok(())
    }

    #[test]
    fn stable_trace_coalesces_warm_build_script_records() -> Result<(), Box<dyn std::error::Error>> {
        let scratch = tempfile::tempdir()?;
        let rustc = scratch.path().join("rustc");
        fs::write(&rustc, b"verified compiler fixture")?;
        let rustc_name = rustc.to_string_lossy().to_string();
        let custom_dir = scratch.path().join("target/debug/build/root-sealed");
        let deps_dir = scratch.path().join("target/debug/deps");
        let output_dir = scratch.path().join("target/debug/build/root-output/out");
        let target_output_dir = scratch.path().join("target/wasm/build/root-output/out");
        fs::create_dir_all(&custom_dir)?;
        fs::create_dir_all(&deps_dir)?;
        fs::create_dir_all(&output_dir)?;
        fs::create_dir_all(&target_output_dir)?;
        let custom_artifact_path = custom_dir.join("build_script_build");
        let consumer_artifact_path = deps_dir.join("libroot-sealed.rlib");
        let target_artifact_path = deps_dir.join("libroot-target.rlib");
        fs::write(&custom_artifact_path, b"custom")?;
        fs::write(&consumer_artifact_path, b"consumer")?;
        fs::write(&target_artifact_path, b"target consumer")?;
        let package_root = scratch.path().join("root");
        fs::create_dir_all(package_root.join("src"))?;
        fs::write(package_root.join("build.rs"), b"fn main() {}\n")?;
        fs::write(package_root.join("src/lib.rs"), b"pub fn root() {}\n")?;
        fs::write(package_root.join("src/probe.rs"), b"pub fn probe() {}\n")?;
        let metadata = serde_json::from_value::<CargoMetadata>(serde_json::json!({
            "packages": [{
                "id": "root 1.0.0", "name": "root", "version": "1.0.0",
                "manifest_path": package_root.join("Cargo.toml")
            }]
        }))?;
        let custom_artifact = serde_json::json!({
            "reason": "compiler-artifact", "package_id": "root 1.0.0",
            "target": {"name": "build-script-build", "kind": ["custom-build"], "crate_types": ["bin"], "src_path": package_root.join("build.rs")},
            "features": [], "filenames": [custom_artifact_path.clone()], "profile": {"test": false}
        });
        let consumer_artifact = serde_json::json!({
            "reason": "compiler-artifact", "package_id": "root 1.0.0",
            "target": {"name": "root", "kind": ["lib"], "crate_types": ["lib"], "src_path": package_root.join("src/lib.rs")},
            "features": [], "filenames": [consumer_artifact_path.clone()], "profile": {"test": false}
        });
        let build_script = serde_json::json!({
            "reason": "build-script-executed", "package_id": "root 1.0.0",
            "out_dir": output_dir.to_string_lossy(), "cfgs": ["sealed"]
        });
        let target_artifact = serde_json::json!({
            "reason": "compiler-artifact", "package_id": "root 1.0.0",
            "target": {"name": "root", "kind": ["lib"], "crate_types": ["lib"], "src_path": package_root.join("src/lib.rs")},
            "features": [], "filenames": [target_artifact_path.clone()], "profile": {"test": false}
        });
        let target_build_script = serde_json::json!({
            "reason": "build-script-executed", "package_id": "root 1.0.0",
            "out_dir": target_output_dir.to_string_lossy(), "cfgs": ["target_sealed"]
        });
        let first = vec![
            custom_artifact.clone(),
            serde_json::json!({
                "reason": "incan-rustc-invocation", "rustc": rustc_name.clone(),
                "arguments": ["--crate-name", "build_script_build", "--crate-type", "bin", "-C", "extra-filename=", "--emit", "link", "--out-dir", custom_dir.to_string_lossy(), package_root.join("build.rs")],
                "environment": {"CARGO_MANIFEST_DIR": package_root.to_string_lossy(), "CARGO_PKG_NAME": "root", "CARGO_PKG_VERSION": "1.0.0"}
            }),
            build_script.clone(),
            serde_json::json!({
                "reason": "incan-rustc-invocation", "rustc": rustc_name.clone(),
                "working_directory": package_root.to_string_lossy(),
                "arguments": ["--cfg=build_script_probe", "--crate-name=root", "--crate-type=lib", "--emit=dep-info,metadata", "--out-dir", output_dir.join("probe").to_string_lossy(), "src/probe.rs"],
                "environment": {"CARGO_MANIFEST_DIR": package_root.to_string_lossy(), "CARGO_PKG_NAME": "root", "CARGO_PKG_VERSION": "1.0.0", "OUT_DIR": output_dir.to_string_lossy(), "TARGET": "fixture-host"}
            }),
            consumer_artifact.clone(),
            serde_json::json!({
                "reason": "incan-rustc-invocation", "rustc": rustc_name.clone(),
                "arguments": ["--crate-name", "root", "--crate-type", "lib", "-C", "extra-filename=-sealed", "--emit", "link", "--out-dir", deps_dir.to_string_lossy(), package_root.join("src/lib.rs")],
                "environment": {"CARGO_MANIFEST_DIR": package_root.to_string_lossy(), "CARGO_PKG_NAME": "root", "CARGO_PKG_VERSION": "1.0.0", "CARGO_PRIMARY_PACKAGE": "1", "OUT_DIR": output_dir.to_string_lossy(), "TARGET": "fixture-host"}
            }),
            target_build_script.clone(),
            target_artifact.clone(),
            serde_json::json!({
                "reason": "incan-rustc-invocation", "rustc": rustc_name.clone(),
                "arguments": ["--crate-name", "root", "--crate-type", "lib", "--target", "wasm32-unknown-unknown", "-C", "extra-filename=-target", "--emit", "link", "--out-dir", deps_dir.to_string_lossy(), package_root.join("src/lib.rs")],
                "environment": {"CARGO_MANIFEST_DIR": package_root.to_string_lossy(), "CARGO_PKG_NAME": "root", "CARGO_PKG_VERSION": "1.0.0", "CARGO_PRIMARY_PACKAGE": "1", "OUT_DIR": target_output_dir.to_string_lossy(), "TARGET": "wasm32-unknown-unknown"}
            }),
        ];
        let second = vec![
            {
                let mut fresh = custom_artifact;
                fresh["fresh"] = serde_json::Value::Bool(true);
                fresh
            },
            build_script,
            {
                let mut fresh = consumer_artifact;
                fresh["fresh"] = serde_json::Value::Bool(true);
                fresh
            },
            target_build_script,
            {
                let mut fresh = target_artifact;
                fresh["fresh"] = serde_json::Value::Bool(true);
                fresh
            },
        ];
        let encode = |records: &[serde_json::Value]| -> Result<CargoInvocationOutput, serde_json::Error> {
            let mut stdout = Vec::new();
            for record in records {
                stdout.extend_from_slice(&serde_json::to_vec(record)?);
                stdout.push(b'\n');
            }
            Ok(CargoInvocationOutput { stdout })
        };
        let capture = capture_legacy_cargo_selected_units_from_trace(
            &metadata,
            &[encode(&first)?, encode(&second)?],
            &rustc,
            "fixture-host",
        )?;
        assert_eq!(capture.units.len(), 3);
        assert_eq!(capture.units[1].dependencies.len(), 1);
        assert_eq!(capture.units[1].dependencies[0].unit_index, 0);
        let facts = capture.units[1].dependencies[0]
            .build_script
            .as_ref()
            .ok_or("missing warm build-script facts")?;
        assert_eq!(facts.cfgs, ["sealed"]);
        assert_eq!(capture.units[2].dependencies.len(), 1);
        assert_eq!(capture.units[2].dependencies[0].unit_index, 0);
        let target_facts = capture.units[2].dependencies[0]
            .build_script
            .as_ref()
            .ok_or("missing target build-script facts")?;
        assert_eq!(target_facts.cfgs, ["target_sealed"]);
        assert_eq!(target_facts.out_dir, target_output_dir);
        assert_eq!(capture.build_script_tool_probes.len(), 1);
        assert_eq!(capture.build_script_tool_probes[0].package_id, "root 1.0.0");
        assert_eq!(capture.build_script_tool_probes[0].out_dir, output_dir);
        assert_eq!(capture.build_script_tool_probes[0].target_context, "fixture-host");
        assert_eq!(capture.build_script_tool_probes[0].rustc_target, "fixture-host");
        let mut conflicting = second.clone();
        conflicting[1]["cfgs"] = serde_json::json!(["different"]);
        let conflict = capture_legacy_cargo_selected_units_from_trace(
            &metadata,
            &[encode(&first)?, encode(&conflicting)?],
            &rustc,
            "fixture-host",
        );
        assert!(conflict.is_err());
        let mut wrong_probe_target = first.clone();
        wrong_probe_target[3]["environment"]["TARGET"] = serde_json::json!("unselected-target");
        let wrong_target = capture_legacy_cargo_selected_units_from_trace(
            &metadata,
            &[encode(&wrong_probe_target)?],
            &rustc,
            "fixture-host",
        );
        assert!(matches!(wrong_target, Err(error) if error.to_string().contains("matches 0 execution edges")));
        let alternate_custom = custom_dir.join("build_script_build-alternate");
        fs::write(&alternate_custom, b"alternate custom")?;
        let mut ambiguous = first;
        ambiguous.push(serde_json::json!({
            "reason": "compiler-artifact", "package_id": "root 1.0.0",
            "target": {"name": "build-script-build", "kind": ["custom-build"], "crate_types": ["bin"], "src_path": package_root.join("build.rs")},
            "features": [], "filenames": [alternate_custom], "profile": {"test": false}
        }));
        ambiguous.push(serde_json::json!({
            "reason": "incan-rustc-invocation", "rustc": rustc_name,
            "arguments": ["--crate-name", "build_script_build", "--crate-type", "bin", "-C", "extra-filename=-alternate", "--emit", "link", "--out-dir", custom_dir.to_string_lossy(), package_root.join("build.rs")],
            "environment": {"CARGO_MANIFEST_DIR": package_root.to_string_lossy(), "CARGO_PKG_NAME": "root", "CARGO_PKG_VERSION": "1.0.0"}
        }));
        let ambiguity =
            capture_legacy_cargo_selected_units_from_trace(&metadata, &[encode(&ambiguous)?], &rustc, "fixture-host");
        assert!(matches!(ambiguity, Err(error) if error.to_string().contains("cannot bind consumer")));
        Ok(())
    }
}
