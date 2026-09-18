//! Registry source trees and the sealed registry leaf catalog a publisher stages beside its closure.
//!
//! Registry checksums come from the lock, source trees are copied under the publisher's staging with their
//! digests recorded, and the leaf catalog binds every registry artifact to the plan that sealed it. The publisher
//! that calls them lives in `legacy_cargo.rs`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use super::{
    CargoChecksumLock, CargoCompilerArtifact, CargoInvocationOutput, CargoMetadata, InspectionPackageScope,
    OvenBuildIntent, OvenLegacyCargoError, OvenLegacyCargoInspectionPackage, OvenLegacyCargoInspectionSourceMember,
    OvenRustcArtifactExtern, OvenRustcRegistryLeaf, OvenRustcRegistryLeafDomain, OvenRustcRegistryLeafKind,
    OvenRustcRegistrySource, OvenRustcRegistrySourcePackage, OvenRustcSupportingArtifact, PendingRegistryLeaf,
    canonical_directory, compiler_artifact_platform, copy_regular_directory_tree, digest_bytes, digest_source_tree,
    inspection_package_closure_ids, legacy_cargo_inspection_sources_from_metadata, materialized_files_from_directory,
    regular_file_bytes, relative_path,
};

/// Decode exact registry checksums from the lock consumed by the named publisher.
pub fn cargo_registry_checksums(
    cargo_lock: &[u8],
) -> Result<BTreeMap<(String, String, String), String>, OvenLegacyCargoError> {
    let lock = toml::from_slice::<CargoChecksumLock>(cargo_lock).map_err(|error| {
        OvenLegacyCargoError::Plan(format!("publisher Cargo.lock is not valid checksum authority: {error}"))
    })?;
    let mut checksums = BTreeMap::new();
    for package in lock.package {
        let Some(registry) = package.source.filter(|source| source.starts_with("registry+")) else {
            continue;
        };
        let checksum = package
            .checksum
            .filter(|checksum| !checksum.trim().is_empty())
            .ok_or_else(|| {
                OvenLegacyCargoError::Plan(format!(
                    "publisher Cargo.lock omits the checksum for registry package `{}` {}",
                    package.name, package.version
                ))
            })?;
        let key = (package.name, package.version, registry);
        if let Some(previous) = checksums.insert(key.clone(), checksum.clone())
            && previous != checksum
        {
            return Err(OvenLegacyCargoError::Plan(format!(
                "publisher Cargo.lock contains conflicting checksums for `{}` {} from `{}`",
                key.0, key.1, key.2
            )));
        }
    }
    Ok(checksums)
}

/// Copy one exact registry package into publisher staging and declare every retained source file.
pub fn stage_registry_source(
    staging: &Path,
    package: &str,
    version: &str,
    registry: &str,
    checksum: &str,
    source_root: &Path,
    source_artifacts: &mut Vec<OvenRustcSupportingArtifact>,
) -> Result<OvenRustcRegistrySource, OvenLegacyCargoError> {
    let (staged_root, digest, _) =
        stage_registry_source_directory(staging, package, version, registry, checksum, source_root)?;
    let relative_root = staged_root
        .strip_prefix(staging)
        .map_err(|_| OvenLegacyCargoError::Plan("staged registry source escaped publisher staging".to_string()))?
        .to_string_lossy()
        .replace('\\', "/");
    for file in materialized_files_from_directory(&staged_root, &relative_root, "registry package source")? {
        source_artifacts.push(OvenRustcSupportingArtifact {
            relative_path: file.relative_path,
            digest: digest_bytes(&regular_file_bytes(&file.source_path)?),
        });
    }
    Ok(OvenRustcRegistrySource {
        registry: registry.to_string(),
        checksum: checksum.to_string(),
        relative_root,
        digest,
    })
}

/// Copy one registry package into private baker state without Cargo's mutable package-local target cache.
pub fn stage_registry_source_directory(
    staging: &Path,
    package: &str,
    version: &str,
    registry: &str,
    checksum: &str,
    source_root: &Path,
) -> Result<(PathBuf, String, Vec<OvenLegacyCargoInspectionSourceMember>), OvenLegacyCargoError> {
    let source_root = canonical_directory(source_root, "registry package source")?;
    let identity = digest_bytes(format!("{registry}\0{package}\0{version}\0{checksum}").as_bytes());
    let identity = identity.strip_prefix("sha256:").unwrap_or(&identity);
    let staged_root = staging.join("registry-sources").join(identity);
    if !staged_root.exists() {
        copy_registry_source_tree(&source_root, &staged_root)?;
    }
    let digest = digest_source_tree(&staged_root).map_err(|error| {
        OvenLegacyCargoError::Plan(format!(
            "could not digest staged registry package `{package}` {version}: {error}"
        ))
    })?;
    let members = materialized_files_from_directory(&staged_root, "", "staged registry package source")?
        .into_iter()
        .map(|file| {
            let path = file.relative_path.strip_prefix('/').ok_or_else(|| {
                OvenLegacyCargoError::Plan("staged registry member path lost its package-relative prefix".to_string())
            })?;
            Ok(OvenLegacyCargoInspectionSourceMember {
                path: path.to_string(),
                digest: digest_bytes(&regular_file_bytes(&file.source_path)?),
            })
        })
        .collect::<Result<Vec<_>, OvenLegacyCargoError>>()?;
    if members.is_empty()
        || members.windows(2).any(|pair| pair[0].path >= pair[1].path)
        || !members.iter().any(|member| member.path == "Cargo.toml")
    {
        return Err(OvenLegacyCargoError::Plan(
            "staged registry package source has no complete ordered Cargo.toml/member inventory".to_string(),
        ));
    }
    Ok((staged_root, digest, members))
}

/// Copy registry package source while excluding mutable output that is not part of the package archive.
pub fn copy_registry_source_tree(source_root: &Path, destination_root: &Path) -> Result<(), OvenLegacyCargoError> {
    let source_root = canonical_directory(source_root, "registry package source")?;
    fs::create_dir_all(destination_root).map_err(|source| OvenLegacyCargoError::Io {
        path: destination_root.to_path_buf(),
        source,
    })?;
    let mut entries = fs::read_dir(&source_root)
        .map_err(|source| OvenLegacyCargoError::Io {
            path: source_root.clone(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| OvenLegacyCargoError::Io {
            path: source_root.clone(),
            source,
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if entry.file_name() == "target" {
            continue;
        }
        let source = entry.path();
        let destination = destination_root.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source).map_err(|source_error| OvenLegacyCargoError::Io {
            path: source.clone(),
            source: source_error,
        })?;
        if metadata.file_type().is_symlink() {
            return Err(OvenLegacyCargoError::InvalidInput {
                field: "registry package source",
                message: format!("refuses symlinked publisher input {}", source.display()),
            });
        }
        if metadata.is_dir() {
            copy_regular_directory_tree(&source, &destination, "registry package source")?;
        } else if metadata.is_file() {
            fs::copy(&source, &destination).map_err(|source_error| OvenLegacyCargoError::Io {
                path: source,
                source: source_error,
            })?;
        } else {
            return Err(OvenLegacyCargoError::InvalidInput {
                field: "registry package source",
                message: format!("refuses non-regular publisher input {}", source.display()),
            });
        }
    }
    Ok(())
}

/// Build the plan's complete sealed source authority independently from its linkable registry leaves.
///
/// A transitive procedural macro may be required by rust-analyzer's source graph without producing an `.rlib` that
/// normal dependency selection can expose. Typed Loaf envelopes therefore retain the complete checked inspection
/// closure here; the older broad transitional request continues to derive source authority from its compiled leaves.
pub fn publisher_registry_source_catalog(
    metadata: &CargoMetadata,
    cargo_lock: &[u8],
    staging: &Path,
    inspection_packages: Option<&[OvenLegacyCargoInspectionPackage]>,
    registry_leaves: &[OvenRustcRegistryLeaf],
    complete_resolved_source_catalog: bool,
    platform_applicable_metadata: Option<&CargoMetadata>,
) -> Result<(Vec<OvenRustcRegistrySourcePackage>, Vec<OvenRustcSupportingArtifact>), OvenLegacyCargoError> {
    let mut sources = registry_leaves
        .iter()
        .map(|leaf| OvenRustcRegistrySourcePackage {
            package: leaf.package.clone(),
            version: leaf.version.clone(),
            features: leaf.features.clone(),
            source: leaf.source.clone(),
        })
        .collect::<Vec<_>>();
    let inspection_sources = match inspection_packages {
        Some(inspection_packages) => Some(legacy_cargo_inspection_sources_from_metadata(
            metadata,
            cargo_lock,
            inspection_packages,
            InspectionPackageScope::CompleteResolvedGraph,
            staging,
        )?),
        // The complete-graph catalog seals a single target's build. Prefer the platform-filtered resolve graph so
        // this closure matches the packages Cargo actually selected for that target, rather than every platform's
        // locked closure; a target-inapplicable package (for example a Linux-only transitive dependency while
        // baking for macOS) must not be required to carry source authority it was never built with.
        None if complete_resolved_source_catalog => Some(legacy_cargo_inspection_sources_from_metadata(
            platform_applicable_metadata.unwrap_or(metadata),
            cargo_lock,
            &[],
            InspectionPackageScope::CompleteResolvedGraph,
            staging,
        )?),
        None => None,
    };
    let Some(inspection_sources) = inspection_sources else {
        sources.sort_by(|left, right| (&left.package, &left.version).cmp(&(&right.package, &right.version)));
        return Ok((sources, Vec::new()));
    };
    let mut source_artifacts = Vec::new();
    for source in inspection_sources {
        let relative_root = source
            .source_root
            .strip_prefix(staging)
            .map_err(|_| OvenLegacyCargoError::Plan("staged registry source escaped publisher staging".to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        for file in
            materialized_files_from_directory(&source.source_root, &relative_root, "registry inspection source")?
        {
            source_artifacts.push(OvenRustcSupportingArtifact {
                relative_path: file.relative_path,
                digest: digest_bytes(&regular_file_bytes(&file.source_path)?),
            });
        }
        sources.push(OvenRustcRegistrySourcePackage {
            package: source.package,
            version: source.version,
            features: source.features,
            source: OvenRustcRegistrySource {
                registry: source.registry,
                checksum: source.checksum,
                relative_root,
                digest: source.source_digest,
            },
        });
    }
    sources.sort_by(|left, right| {
        (
            &left.package,
            &left.version,
            &left.source.registry,
            &left.source.checksum,
        )
            .cmp(&(
                &right.package,
                &right.version,
                &right.source.registry,
                &right.source.checksum,
            ))
    });
    sources.dedup_by(|left, right| {
        left.package == right.package
            && left.version == right.version
            && left.source.registry == right.source.registry
            && left.source.checksum == right.source.checksum
    });
    source_artifacts.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    source_artifacts.dedup_by(|left, right| left.relative_path == right.relative_path && left.digest == right.digest);
    Ok((sources, source_artifacts))
}

/// Retain exact registry leaves that the named publisher actually compiled into one Loaf.
///
/// Cargo's JSON artifact record is correlated with its publisher-only metadata package record while both are still
/// inside the explicit transition boundary. The resulting catalog names a single checked artifact already retained by
/// the direct-Rustc plan; it is not a registry index and cannot trigger source discovery or Cargo at consumption.
pub struct PublisherRegistryLeafCatalogRequest<'a> {
    pub outputs: &'a [CargoInvocationOutput],
    pub metadata: &'a CargoMetadata,
    pub cargo_lock: &'a [u8],
    pub staging: &'a Path,
    pub intent: &'a OvenBuildIntent,
    pub externs: &'a [OvenRustcArtifactExtern],
    pub supporting_artifacts: &'a [OvenRustcSupportingArtifact],
    pub selected_units: Option<&'a super::OvenLegacyCargoSelectedUnitCapture>,
    pub inspection_packages: Option<&'a [OvenLegacyCargoInspectionPackage]>,
}

/// Build the immutable registry-leaf catalog from one explicit publisher result.
pub fn publisher_registry_leaf_catalog(
    request: PublisherRegistryLeafCatalogRequest<'_>,
) -> Result<(Vec<OvenRustcRegistryLeaf>, Vec<OvenRustcSupportingArtifact>), OvenLegacyCargoError> {
    let PublisherRegistryLeafCatalogRequest {
        outputs,
        metadata,
        cargo_lock,
        staging,
        intent,
        externs,
        supporting_artifacts,
        selected_units,
        inspection_packages,
    } = request;
    let mut retained = BTreeMap::<String, String>::new();
    for artifact in externs {
        retained.insert(artifact.relative_path.clone(), artifact.digest.clone());
    }
    for artifact in supporting_artifacts {
        retained.insert(artifact.relative_path.clone(), artifact.digest.clone());
    }
    let packages = metadata
        .packages
        .iter()
        .map(|package| (package.id.as_str(), package))
        .collect::<BTreeMap<_, _>>();
    let checksums = cargo_registry_checksums(cargo_lock)?;
    let selected_package_ids = inspection_packages
        .map(|packages| inspection_package_closure_ids(metadata, packages, InspectionPackageScope::DirectRoot))
        .transpose()?;
    let mut candidates = Vec::<PendingRegistryLeaf>::new();
    for output in outputs {
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let Ok(artifact) = serde_json::from_str::<CargoCompilerArtifact>(line) else {
                continue;
            };
            if artifact.reason != "compiler-artifact" {
                continue;
            }
            if selected_package_ids
                .as_ref()
                .is_some_and(|selected| !selected.contains(&artifact.package_id))
            {
                continue;
            }
            let Some(package) = packages.get(artifact.package_id.as_str()) else {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "named Loaf publisher emitted package `{}` absent from its metadata",
                    artifact.package_id
                )));
            };
            let Some(registry) = package
                .source
                .as_deref()
                .filter(|source| source.starts_with("registry+"))
            else {
                continue;
            };
            let selected_unit_identity = selected_units
                .map(|selected_units| {
                    let selected = traced_units_for_registry_artifact(selected_units, &artifact);
                    let [selected] = selected.as_slice() else {
                        return Err(OvenLegacyCargoError::Plan(format!(
                            "registry artifact `{}` {} does not bind exactly one traced rustc output",
                            package.name, package.version
                        )));
                    };
                    super::legacy_cargo_selected_unit_capture_identity(selected)
                })
                .transpose()?;
            // A procedural macro is a host dynamic library rustc loads while compiling its consumer; every other
            // registry unit is a Rust library archive, compiled for the target or, when a macro depends on it, for
            // the build host. Each is sealed as its own leaf, labelled by domain and kind, so the catalog keeps a
            // package's host and target compilations apart instead of dropping one.
            let proc_macro = artifact.target.kind.iter().any(|kind| kind == "proc-macro");
            let crate_kind = if proc_macro {
                OvenRustcRegistryLeafKind::ProcMacro
            } else {
                OvenRustcRegistryLeafKind::Rlib
            };
            let mut artifacts = artifact
                .filenames
                .into_iter()
                .filter(|path| {
                    let extension = path.extension().and_then(|extension| extension.to_str());
                    if proc_macro {
                        matches!(extension, Some("dylib" | "so" | "dll"))
                    } else {
                        extension == Some("rlib")
                    }
                })
                .filter_map(|path| {
                    let canonical = fs::canonicalize(&path).ok()?;
                    let target_artifact = !proc_macro
                        && compiler_artifact_platform(&canonical, &intent.target) == Some(intent.target.clone());
                    let domain = if target_artifact {
                        OvenRustcRegistryLeafDomain::Target
                    } else {
                        OvenRustcRegistryLeafDomain::Host
                    };
                    Some((canonical, domain))
                })
                .filter_map(|(path, domain)| {
                    let relative = relative_path(staging, &path).ok()?;
                    retained
                        .get(&relative)
                        .cloned()
                        .map(|digest| (relative, digest, domain))
                })
                .collect::<Vec<_>>();
            artifacts.sort();
            artifacts.dedup();
            let Some((relative_path, digest, domain)) = artifacts.as_slice().first().cloned() else {
                continue;
            };
            if artifacts.len() != 1 {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "named Loaf publisher emitted multiple retained artifacts for registry package `{}` {}",
                    package.name, package.version
                )));
            }
            let crate_name = artifact.target.name.replace('-', "_");
            let mut features = artifact.features;
            features.sort();
            features.dedup();
            let checksum = checksums
                .get(&(package.name.clone(), package.version.clone(), registry.to_string()))
                .cloned()
                .ok_or_else(|| {
                    OvenLegacyCargoError::Plan(format!(
                        "publisher lock has no checksum for registry package `{}` {} from `{registry}`",
                        package.name, package.version
                    ))
                })?;
            let source_root = package
                .manifest_path
                .parent()
                .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                    field: "registry package manifest",
                    message: format!("{} has no package root", package.manifest_path.display()),
                })?
                .to_path_buf();
            let leaf = PendingRegistryLeaf {
                selected_unit_identity,
                package: package.name.clone(),
                version: package.version.clone(),
                crate_name: crate_name.clone(),
                features,
                artifact: OvenRustcArtifactExtern {
                    crate_name: crate_name.clone(),
                    relative_path,
                    digest,
                },
                registry: registry.to_string(),
                checksum,
                source_root,
                domain,
                crate_kind,
            };
            candidates.push(leaf);
        }
    }
    candidates.sort_by(|left, right| {
        (
            left.package.as_str(),
            left.version.as_str(),
            left.crate_name.as_str(),
            left.domain,
            left.crate_kind,
            left.artifact.relative_path.as_str(),
        )
            .cmp(&(
                right.package.as_str(),
                right.version.as_str(),
                right.crate_name.as_str(),
                right.domain,
                right.crate_kind,
                right.artifact.relative_path.as_str(),
            ))
    });
    type LeafKey = (
        String,
        String,
        String,
        OvenRustcRegistryLeafDomain,
        OvenRustcRegistryLeafKind,
    );
    let mut leaves = BTreeMap::<LeafKey, PendingRegistryLeaf>::new();
    for leaf in candidates {
        let key = (
            leaf.package.clone(),
            leaf.version.clone(),
            leaf.crate_name.clone(),
            leaf.domain,
            leaf.crate_kind,
        );
        match leaves.get(&key) {
            Some(existing)
                if existing.artifact == leaf.artifact
                    && existing.features == leaf.features
                    && existing.registry == leaf.registry
                    && existing.checksum == leaf.checksum
                    && existing.source_root == leaf.source_root => {}
            Some(existing) => {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "named Loaf publisher emitted conflicting registry leaf `{}` {}: {} and {}",
                    leaf.package, leaf.version, existing.artifact.relative_path, leaf.artifact.relative_path
                )));
            }
            None => {
                leaves.insert(key, leaf);
            }
        }
    }
    let mut source_artifacts = Vec::new();
    let mut sealed = Vec::new();
    for leaf in leaves.into_values() {
        let source = stage_registry_source(
            staging,
            &leaf.package,
            &leaf.version,
            &leaf.registry,
            &leaf.checksum,
            &leaf.source_root,
            &mut source_artifacts,
        )?;
        sealed.push(OvenRustcRegistryLeaf {
            selected_unit_identity: leaf.selected_unit_identity,
            domain: leaf.domain,
            crate_kind: leaf.crate_kind,
            package: leaf.package,
            version: leaf.version,
            crate_name: leaf.crate_name,
            features: leaf.features,
            source,
            artifact: leaf.artifact,
        });
    }
    source_artifacts.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    source_artifacts.dedup_by(|left, right| left.relative_path == right.relative_path && left.digest == right.digest);
    Ok((sealed, source_artifacts))
}

/// Find physical units whose exact traced output set intersects one Cargo artifact record.
fn traced_units_for_registry_artifact<'a>(
    selected_units: &'a super::OvenLegacyCargoSelectedUnitCapture,
    artifact: &CargoCompilerArtifact,
) -> Vec<&'a super::OvenLegacyCargoSelectedUnit> {
    let reported_paths = artifact
        .filenames
        .iter()
        .filter_map(|path| fs::canonicalize(path).ok())
        .collect::<BTreeSet<_>>();
    selected_units
        .units
        .iter()
        .filter(|unit| {
            unit.package_id == artifact.package_id
                && unit.target_name == artifact.target.name
                && unit.artifact_paths.iter().any(|path| {
                    fs::canonicalize(path)
                        .ok()
                        .is_some_and(|path| reported_paths.contains(&path))
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::{
        CargoCompilerArtifact, digest_bytes, stage_registry_source_directory, traced_units_for_registry_artifact,
    };
    use crate::{
        CargoCompilerArtifactProfile, CargoCompilerArtifactTarget, OvenLegacyCargoSelectedUnit,
        OvenLegacyCargoSelectedUnitCapture,
    };

    fn selected_unit(package_id: &str, output: PathBuf, platform: &str, cfg: &str) -> OvenLegacyCargoSelectedUnit {
        OvenLegacyCargoSelectedUnit {
            package_id: package_id.to_string(),
            package: "fixture".to_string(),
            package_version: "1.0.0".to_string(),
            package_source: Some("registry+https://example.invalid/index".to_string()),
            target_name: "fixture".to_string(),
            target_kinds: vec!["lib".to_string()],
            crate_types: vec!["lib".to_string()],
            source_path: PathBuf::from("/sealed/fixture/src/lib.rs"),
            artifact_paths: vec![output],
            root_module: "src/lib.rs".to_string(),
            edition: "2021".to_string(),
            mode: "build".to_string(),
            platform: Some(platform.to_string()),
            target_is_explicit: Some(true),
            cfg: vec![cfg.to_string()],
            effective_features: vec!["std".to_string()],
            dependencies: Vec::new(),
            sysroot_externs: Vec::new(),
            build_script: None,
            registry_source: None,
        }
    }

    #[test]
    fn registry_artifact_joins_only_its_exact_traced_output() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let host_output = root.path().join("host/libfixture.rlib");
        let target_output = root.path().join("target/libfixture.rlib");
        fs::create_dir_all(host_output.parent().ok_or("host output has no parent")?)?;
        fs::create_dir_all(target_output.parent().ok_or("target output has no parent")?)?;
        fs::write(&host_output, b"host")?;
        fs::write(&target_output, b"target")?;
        let package_id = "registry+https://example.invalid/index#fixture@1.0.0";
        let capture = OvenLegacyCargoSelectedUnitCapture {
            roots: vec![0, 1],
            units: vec![
                selected_unit(package_id, host_output.clone(), "aarch64-apple-darwin", "host"),
                selected_unit(package_id, target_output.clone(), "wasm32-unknown-unknown", "target"),
            ],
            rustc_invocations_observed: true,
            build_script_tool_probes: Vec::new(),
            compiler: None,
        };
        let artifact = |filenames| CargoCompilerArtifact {
            reason: "compiler-artifact".to_string(),
            package_id: package_id.to_string(),
            target: CargoCompilerArtifactTarget {
                name: "fixture".to_string(),
                kind: vec!["lib".to_string()],
                crate_types: vec!["lib".to_string()],
                src_path: PathBuf::from("/sealed/fixture/src/lib.rs"),
            },
            features: vec!["std".to_string()],
            filenames,
            profile: CargoCompilerArtifactProfile::default(),
        };

        assert_eq!(
            traced_units_for_registry_artifact(&capture, &artifact(vec![target_output])).len(),
            1
        );
        assert!(
            traced_units_for_registry_artifact(&capture, &artifact(vec![root.path().join("wrong.rlib")])).is_empty()
        );
        assert_eq!(
            traced_units_for_registry_artifact(
                &capture,
                &artifact(vec![host_output, root.path().join("target/libfixture.rlib")])
            )
            .len(),
            2
        );
        Ok(())
    }

    #[test]
    fn staged_registry_source_retains_sorted_cargo_toml_and_member_digests() -> Result<(), Box<dyn std::error::Error>> {
        let source = tempfile::tempdir()?;
        let staging = tempfile::tempdir()?;
        fs::create_dir_all(source.path().join("src"))?;
        fs::write(
            source.path().join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"1.0.0\"\n",
        )?;
        fs::write(source.path().join("src/lib.rs"), "pub fn marker() {}\n")?;
        fs::write(
            source.path().join("src.rs"),
            "// file sorts before the src directory contents\n",
        )?;

        let (_, _, members) = stage_registry_source_directory(
            staging.path(),
            "fixture",
            "1.0.0",
            "registry+https://example.invalid/index",
            "fixture-checksum",
            source.path(),
        )?;

        assert_eq!(
            members.iter().map(|member| member.path.as_str()).collect::<Vec<_>>(),
            ["Cargo.toml", "src.rs", "src/lib.rs"],
        );
        assert_eq!(
            members.iter().map(|member| member.digest.as_str()).collect::<Vec<_>>(),
            [
                digest_bytes(b"[package]\nname = \"fixture\"\nversion = \"1.0.0\"\n"),
                digest_bytes(b"// file sorts before the src directory contents\n"),
                digest_bytes(b"pub fn marker() {}\n"),
            ],
        );
        Ok(())
    }
}
