//! Cargo lock graphs and the release-cohort lock policy the explicit publisher enforces.
//!
//! A generated project's `Cargo.lock` is seeded from the checked compiler lock and must stay inside the release
//! cohort it names: every registry pin the compiler ships is inherited exactly, and only project-only edges may
//! resolve fresh. These helpers decode lock documents into package graphs, validate a generated lock against the
//! compiler's, and prune a staged lock down to one package's closure. The publisher that calls them lives in
//! `legacy_cargo.rs`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::cargo_json::CargoChecksumLock;
use super::{OvenLegacyCargoError, cargo_registry_checksums, locked_local_package};

pub(crate) type CargoLockPackageIdentity = (String, String, Option<String>);

/// One resolved package node from a Cargo lock graph.
pub(crate) struct CargoLockPackageNode {
    checksum: Option<String>,
    dependencies: BTreeSet<CargoLockPackageIdentity>,
}

/// Decode a Cargo lock into exact package identities and resolved dependency edges.
pub(crate) fn cargo_lock_package_graph(
    cargo_lock: &[u8],
    field: &'static str,
) -> Result<BTreeMap<CargoLockPackageIdentity, CargoLockPackageNode>, OvenLegacyCargoError> {
    let lock = toml::from_slice::<toml::Value>(cargo_lock).map_err(|error| OvenLegacyCargoError::InvalidInput {
        field,
        message: error.to_string(),
    })?;
    let packages =
        lock.get("package")
            .and_then(toml::Value::as_array)
            .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                field,
                message: "must contain a package array".to_string(),
            })?;
    let identities = lock_package_identities(&lock);
    if packages.len() != identities.len() {
        return Err(OvenLegacyCargoError::InvalidInput {
            field,
            message: "every package must declare a name and version".to_string(),
        });
    }
    if identities.iter().collect::<BTreeSet<_>>().len() != identities.len() {
        return Err(OvenLegacyCargoError::InvalidInput {
            field,
            message: "must not contain duplicate package identities".to_string(),
        });
    }
    let mut references = BTreeMap::new();
    for identity in &identities {
        let same_name = identities.iter().filter(|candidate| candidate.0 == identity.0).count();
        let same_name_version = identities
            .iter()
            .filter(|candidate| candidate.0 == identity.0 && candidate.1 == identity.1)
            .count();
        let mut aliases = Vec::new();
        if same_name == 1 {
            aliases.push(identity.0.clone());
        }
        if same_name_version == 1 {
            aliases.push(format!("{} {}", identity.0, identity.1));
        }
        if let Some(source) = identity.2.as_deref() {
            aliases.push(format!("{} {} ({source})", identity.0, identity.1));
        }
        for reference in aliases {
            if let Some(previous) = references.insert(reference.clone(), identity.clone())
                && previous != *identity
            {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "{field} has ambiguous dependency reference `{reference}`"
                )));
            }
        }
    }
    let mut graph = BTreeMap::new();
    for (package, identity) in packages.iter().zip(identities) {
        let dependencies = package
            .get("dependencies")
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
            .map(|dependency| {
                let dependency = dependency.as_str().ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                    field,
                    message: "package dependency must be a string".to_string(),
                })?;
                references.get(dependency).cloned().ok_or_else(|| {
                    OvenLegacyCargoError::Plan(format!("{field} dependency `{dependency}` has no exact package record"))
                })
            })
            .collect::<Result<BTreeSet<_>, _>>()?;
        let node = CargoLockPackageNode {
            checksum: package
                .get("checksum")
                .and_then(toml::Value::as_str)
                .map(str::to_string),
            dependencies,
        };
        if graph.insert(identity.clone(), node).is_some() {
            return Err(OvenLegacyCargoError::Plan(format!(
                "{field} contains duplicate package `{}` {} from {:?}",
                identity.0, identity.1, identity.2
            )));
        }
    }
    Ok(graph)
}

/// Verify that Cargo retained the seeded release graph while admitting project-only registry coordinates.
///
/// Package names are not globally owned by a release: a project dependency may legitimately require an incompatible
/// version of a package also used by the standard library. The invariant is instead graph-shaped. Every package and
/// dependency edge on a release-owned local/path node retained by the generated root must remain a subset of the
/// release graph because its compiled artifact is later replaced by the complete release copy. Cargo may prune
/// feature-disabled local edges, normalize target- and feature-sensitive dependency lists on registry nodes, or
/// prune unreachable release nodes; their exact coordinate/checksum and the later feature-bound artifact catalog
/// remain the authority. Extra registry coordinates remain project-owned.
pub(crate) fn validate_release_cohort_registry_lock(
    release_lock: &[u8],
    seeded_lock: &[u8],
    generated_lock: &[u8],
) -> Result<(), OvenLegacyCargoError> {
    let release_checksums = cargo_registry_checksums(release_lock)?;
    let release = cargo_lock_package_graph(release_lock, "selected release Cargo.lock")?;
    let seeded = cargo_lock_package_graph(seeded_lock, "release-derived project Cargo.lock")?;
    let generated = cargo_lock_package_graph(generated_lock, "generated project Cargo.lock")?;

    for (stage, graph) in [("release-derived project", &seeded), ("generated project", &generated)] {
        for (identity, node) in graph {
            let Some(release_node) = release.get(identity) else {
                continue;
            };
            let is_registry = identity
                .2
                .as_deref()
                .is_some_and(|source| source.starts_with("registry+"));
            if !is_registry && !node.dependencies.is_subset(&release_node.dependencies) {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "{stage} lock changed the release-derived dependency edges for `{}` {}",
                    identity.0, identity.1,
                )));
            }
            if is_registry && node.checksum != release_node.checksum {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "{stage} lock changed the release-derived checksum for `{}` {}",
                    identity.0, identity.1,
                )));
            }
        }
    }

    for (identity, generated_node) in &generated {
        let Some(source) = identity.2.as_deref() else {
            continue;
        };
        if !source.starts_with("registry+") {
            return Err(OvenLegacyCargoError::Plan(format!(
                "generated project lock selected unsupported source `{source}` for `{}` {}",
                identity.0, identity.1
            )));
        }
        let Some(release_checksum) =
            release_checksums.get(&(identity.0.clone(), identity.1.clone(), source.to_string()))
        else {
            continue;
        };
        if generated_node.checksum.as_ref() != Some(release_checksum) {
            return Err(OvenLegacyCargoError::Plan(format!(
                "generated project lock checksum for release coordinate `{}` {} disagrees with the selected release cohort",
                identity.0, identity.1
            )));
        }
    }
    Ok(())
}

/// Prove that local Cargo normalization introduced no registry identity outside the checked compiler lock.
pub(crate) fn validate_generated_registry_lock(
    compiler_lock: &[u8],
    generated_lock: &[u8],
) -> Result<(), OvenLegacyCargoError> {
    let compiler_checksums = cargo_registry_checksums(compiler_lock)?;
    let generated = toml::from_slice::<CargoChecksumLock>(generated_lock).map_err(|error| {
        OvenLegacyCargoError::Plan(format!(
            "generated Loaf Cargo.lock is not valid checksum authority: {error}"
        ))
    })?;
    for package in generated.package {
        let Some(source) = package.source else {
            continue;
        };
        if !source.starts_with("registry+") {
            return Err(OvenLegacyCargoError::Plan(format!(
                "generated Loaf lock selected unsupported source `{source}` for `{}` {}",
                package.name, package.version
            )));
        }
        let checksum = package
            .checksum
            .filter(|checksum| !checksum.trim().is_empty())
            .ok_or_else(|| {
                OvenLegacyCargoError::Plan(format!(
                    "generated Loaf lock omits the checksum for registry package `{}` {}",
                    package.name, package.version
                ))
            })?;
        let key = (package.name, package.version, source);
        let Some(compiler_checksum) = compiler_checksums.get(&key) else {
            return Err(OvenLegacyCargoError::Plan(format!(
                "generated Loaf lock selected registry package `{}` {} outside the checked compiler lock",
                key.0, key.1
            )));
        };
        if compiler_checksum != &checksum {
            return Err(OvenLegacyCargoError::Plan(format!(
                "generated Loaf lock checksum for `{}` {} disagrees with the checked compiler lock",
                key.0, key.1
            )));
        }
    }
    Ok(())
}

/// Extend one checked compiler lock with the local packages reachable from a generated Cargo manifest.
pub(crate) fn locked_generated_project(
    manifest_path: &Path,
    manifest: &[u8],
    source_lock: &[u8],
) -> Result<Vec<u8>, OvenLegacyCargoError> {
    locked_generated_project_with_registry_policy(manifest_path, manifest, source_lock, false)
}

/// Seed a generated project from one release lock while allowing genuinely project-owned registry packages.
pub(crate) fn release_cohort_generated_project_lock(
    manifest_path: &Path,
    manifest: &[u8],
    source_lock: &[u8],
) -> Result<Vec<u8>, OvenLegacyCargoError> {
    locked_generated_project_with_registry_policy(manifest_path, manifest, source_lock, true)
}

/// Extend one checked release lock with local packages reachable from a generated Cargo manifest.
pub(crate) fn locked_generated_project_with_registry_policy(
    manifest_path: &Path,
    manifest: &[u8],
    source_lock: &[u8],
    allow_project_registry_packages: bool,
) -> Result<Vec<u8>, OvenLegacyCargoError> {
    let source_lock = std::str::from_utf8(source_lock).map_err(|error| OvenLegacyCargoError::InvalidInput {
        field: "compiler Cargo.lock",
        message: error.to_string(),
    })?;
    let mut lock = toml::from_str::<toml::Value>(source_lock).map_err(|error| OvenLegacyCargoError::InvalidInput {
        field: "compiler Cargo.lock",
        message: error.to_string(),
    })?;
    let manifest_text = std::str::from_utf8(manifest).map_err(|error| OvenLegacyCargoError::InvalidInput {
        field: "generated Loaf Cargo.toml",
        message: error.to_string(),
    })?;
    let manifest =
        toml::from_str::<toml::Value>(manifest_text).map_err(|error| OvenLegacyCargoError::InvalidInput {
            field: "generated Loaf Cargo.toml",
            message: error.to_string(),
        })?;
    let mut visiting = BTreeSet::new();
    let root = locked_local_package(
        manifest_path,
        &manifest,
        &mut lock,
        &mut visiting,
        true,
        allow_project_registry_packages,
    )?;
    let packages = lock
        .get_mut("package")
        .and_then(toml::Value::as_array_mut)
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "compiler Cargo.lock",
            message: "must contain a package array".to_string(),
        })?;
    packages.retain(|package| {
        !(package.get("name").and_then(toml::Value::as_str) == Some(root.name.as_str())
            && package.get("version").and_then(toml::Value::as_str) == Some(root.version.as_str())
            && package.get("source").is_none())
    });
    packages.push(root.value);
    prune_lock_to_package(&mut lock, &root.name, &root.version, None)?;
    toml::to_string_pretty(&lock)
        .map(String::into_bytes)
        .map_err(|error| OvenLegacyCargoError::InvalidInput {
            field: "generated Loaf Cargo.lock",
            message: error.to_string(),
        })
}

/// Decode package identities from one Cargo lock document.
pub(crate) fn lock_package_identities(lock: &toml::Value) -> Vec<(String, String, Option<String>)> {
    lock.get("package")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|package| {
            Some((
                package.get("name")?.as_str()?.to_string(),
                package.get("version")?.as_str()?.to_string(),
                package.get("source").and_then(toml::Value::as_str).map(str::to_string),
            ))
        })
        .collect()
}

/// Return whether one exact package identity is already present in a staged lock.
pub(crate) fn lock_contains_package(lock: &toml::Value, name: &str, version: &str, source: Option<&str>) -> bool {
    lock_package_identities(lock)
        .iter()
        .any(|(candidate_name, candidate_version, candidate_source)| {
            candidate_name == name && candidate_version == version && candidate_source.as_deref() == source
        })
}

/// Render Cargo's shortest unambiguous dependency reference for one exact locked package.
pub(crate) fn lock_package_reference(
    lock: &toml::Value,
    name: &str,
    version: &str,
    source: Option<&str>,
) -> Result<String, OvenLegacyCargoError> {
    let identities = lock_package_identities(lock);
    let exact = identities
        .iter()
        .filter(|(candidate_name, candidate_version, candidate_source)| {
            candidate_name == name && candidate_version == version && candidate_source.as_deref() == source
        })
        .count();
    if exact != 1 {
        return Err(OvenLegacyCargoError::Plan(format!(
            "staged Loaf lock must contain exactly one `{name}` {version} package from {source:?}, found {exact}"
        )));
    }
    let same_name = identities
        .iter()
        .filter(|(candidate_name, _, _)| candidate_name == name)
        .count();
    let same_name_version = identities
        .iter()
        .filter(|(candidate_name, candidate_version, _)| candidate_name == name && candidate_version == version)
        .count();
    Ok(if same_name == 1 {
        name.to_string()
    } else if same_name_version == 1 {
        format!("{name} {version}")
    } else if let Some(source) = source {
        format!("{name} {version} ({source})")
    } else {
        format!("{name} {version}")
    })
}

/// Retain only the package graph reachable from one synthetic root without re-resolving any package identity.
///
/// Cargo regards unreachable package records as a lock-file update. A compiler lock therefore cannot be copied
/// wholesale into a smaller private publisher even when every selected version is correct. Following the lock's own
/// exact dependency references produces the minimal accepted closure while preserving its versions and checksums.
pub(crate) fn prune_lock_to_package(
    lock: &mut toml::Value,
    root_name: &str,
    root_version: &str,
    root_source: Option<&str>,
) -> Result<(), OvenLegacyCargoError> {
    let packages = lock
        .get("package")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "staged Loaf Cargo.lock",
            message: "must contain a package array".to_string(),
        })?
        .clone();
    let identities = lock_package_identities(lock);
    if packages.len() != identities.len() {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "staged Loaf Cargo.lock",
            message: "every package must declare a name and version".to_string(),
        });
    }
    let mut references = BTreeMap::new();
    for (index, (name, version, source)) in identities.iter().enumerate() {
        let reference = lock_package_reference(lock, name, version, source.as_deref())?;
        if references.insert(reference.clone(), index).is_some() {
            return Err(OvenLegacyCargoError::Plan(format!(
                "staged Loaf lock has ambiguous dependency reference `{reference}`"
            )));
        }
        if source.is_none() {
            let versioned_reference = format!("{name} {version}");
            if references
                .insert(versioned_reference.clone(), index)
                .is_some_and(|previous| previous != index)
            {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "staged Loaf lock has ambiguous dependency reference `{versioned_reference}`"
                )));
            }
        }
    }
    let root = identities
        .iter()
        .position(|(name, version, source)| {
            name == root_name && version == root_version && source.as_deref() == root_source
        })
        .ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "staged Loaf lock has no synthetic root `{root_name}` {root_version}"
            ))
        })?;
    let mut reachable = BTreeSet::new();
    let mut pending = vec![root];
    while let Some(index) = pending.pop() {
        if !reachable.insert(index) {
            continue;
        }
        let package = packages.get(index).ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "staged Loaf lock package index {index} is outside its package list"
            ))
        })?;
        for dependency in package
            .get("dependencies")
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
        {
            let dependency = dependency.as_str().ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                field: "staged Loaf Cargo.lock dependency",
                message: "must be a string".to_string(),
            })?;
            let dependency_index = references.get(dependency).copied().ok_or_else(|| {
                OvenLegacyCargoError::Plan(format!(
                    "staged Loaf lock dependency `{dependency}` has no exact package record"
                ))
            })?;
            pending.push(dependency_index);
        }
    }
    let retained = packages
        .into_iter()
        .enumerate()
        .filter_map(|(index, package)| reachable.contains(&index).then_some(package))
        .collect();
    lock.as_table_mut()
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "staged Loaf Cargo.lock",
            message: "must be a TOML table".to_string(),
        })?
        .insert("package".to_string(), toml::Value::Array(retained));
    Ok(())
}
