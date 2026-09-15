//! The local Cargo workspace authority a generated project inherits from the compiler checkout.
//!
//! Locked local packages, inherited workspace versions and merged workspace dependency specifications are what make
//! a generated project's manifest agree with the compiler's own workspace. These helpers read that workspace, digest
//! its authority, and resolve one manifest dependency entry against the compiler lock. The publisher that calls them
//! lives in `legacy_cargo.rs`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use super::{
    OvenLegacyCargoError, digest_bytes, lock_contains_package, lock_package_identities, lock_package_reference,
    regular_file_bytes, verified_regular_file,
};
use oven_model::digest::digest_toolchain_source_tree_with_cache;

/// One local package record ready to append to a staged lock.
pub struct LockedLocalPackage {
    pub name: String,
    pub version: String,
    pub value: toml::Value,
}

/// Cargo workspace authority inherited by one local package manifest.
///
/// The manifest is retained alongside its canonical root so inherited dependency paths are resolved against the
/// workspace that declared them rather than the member that selected them. This effective projection is then folded
/// into the generated lock graph, which binds the selected workspace package and dependency authority into the same
/// identity used by the explicit publisher.
pub struct LocalCargoWorkspaceAuthority {
    root: PathBuf,
    manifest_path: PathBuf,
    manifest: toml::Value,
}

/// One dependency specification after applying Cargo workspace inheritance.
pub struct EffectiveLocalCargoDependency {
    alias: String,
    specification: toml::Value,
    declaration_root: PathBuf,
    inherited_workspace: bool,
}

/// Digest only the effective Cargo workspace facts selected by one local package.
///
/// The package's own tree remains separate source authority. This supplemental identity includes each
/// `[workspace.package]` field explicitly selected with `workspace = true` and every effective
/// `[workspace.dependencies]` declaration selected by ordinary, build, dev, or target-specific dependencies. A
/// selected workspace-relative path dependency contributes its Cargo-semantic source digest instead of a local path.
/// Packages with no inherited workspace facts return `None`, avoiding invalidation from unrelated workspace edits.
pub fn digest_local_cargo_workspace_authority(package_root: &Path) -> Result<Option<String>, OvenLegacyCargoError> {
    let manifest_path = verified_regular_file(
        &package_root.join("Cargo.toml"),
        "local Cargo package workspace authority",
    )?;
    let manifest_bytes = fs::read(&manifest_path).map_err(|source| OvenLegacyCargoError::Io {
        path: manifest_path.clone(),
        source,
    })?;
    let manifest =
        toml::from_slice::<toml::Value>(&manifest_bytes).map_err(|error| OvenLegacyCargoError::InvalidInput {
            field: "local Cargo package workspace authority",
            message: format!("{} is not valid TOML: {error}", manifest_path.display()),
        })?;
    let workspace = local_cargo_workspace_authority(&manifest_path, &manifest)?;
    let mut records = BTreeSet::new();
    if let Some(package) = manifest.get("package").and_then(toml::Value::as_table) {
        for (field, selection) in package {
            let Some(selection) = selection.as_table().and_then(|selection| selection.get("workspace")) else {
                continue;
            };
            if selection.as_bool() != Some(true) {
                return Err(OvenLegacyCargoError::InvalidInput {
                    field: "local Cargo package workspace authority",
                    message: format!(
                        "{} package field `{field}` must set workspace = true to inherit workspace authority",
                        manifest_path.display()
                    ),
                });
            }
            let workspace = workspace.as_ref().ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                field: "local Cargo package workspace authority",
                message: format!(
                    "{} package field `{field}` inherits [workspace.package] but no containing Cargo workspace was found",
                    manifest_path.display()
                ),
            })?;
            let inherited = workspace
                .manifest
                .get("workspace")
                .and_then(toml::Value::as_table)
                .and_then(|workspace| workspace.get("package"))
                .and_then(toml::Value::as_table)
                .and_then(|package| package.get(field))
                .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                    field: "local Cargo package workspace authority",
                    message: format!(
                        "{} has no [workspace.package].{field} inherited by {}",
                        workspace.manifest_path.display(),
                        manifest_path.display()
                    ),
                })?;
            records.insert(format!(
                "package:{field}:{}",
                serde_json::to_string(inherited).map_err(|error| OvenLegacyCargoError::Plan(error.to_string()))?
            ));
        }
    }
    let mut resolved_packages = BTreeMap::new();
    for dependency in manifest_dependency_entries(&manifest_path, &manifest, workspace.as_ref())?
        .into_iter()
        .filter(|dependency| dependency.inherited_workspace)
    {
        let mut specification = dependency.specification;
        if let Some(table) = specification.as_table_mut()
            && let Some(path) = table.get("path").and_then(toml::Value::as_str)
        {
            let dependency_root = dependency.declaration_root.join(path);
            let digest =
                digest_toolchain_source_tree_with_cache(&dependency_root, &mut resolved_packages).map_err(|error| {
                    OvenLegacyCargoError::Plan(format!(
                        "cannot digest inherited workspace dependency `{}` at {}: {error}",
                        dependency.alias,
                        dependency_root.display()
                    ))
                })?;
            table.insert(
                "path".to_string(),
                toml::Value::String(format!("incan-cargo-package:{digest}")),
            );
        }
        records.insert(format!(
            "dependency:{}:{}",
            dependency.alias,
            serde_json::to_string(&specification).map_err(|error| OvenLegacyCargoError::Plan(error.to_string()))?
        ));
    }
    if records.is_empty() {
        return Ok(None);
    }
    let payload = serde_json::to_vec(&records).map_err(|error| OvenLegacyCargoError::Plan(error.to_string()))?;
    Ok(Some(digest_bytes(&payload)))
}

/// Materialize one local package record and recursively add path dependencies missing from the compiler lock.
pub fn locked_local_package(
    manifest_path: &Path,
    manifest: &toml::Value,
    lock: &mut toml::Value,
    visiting: &mut BTreeSet<PathBuf>,
    force_record: bool,
    allow_project_registry_packages: bool,
) -> Result<LockedLocalPackage, OvenLegacyCargoError> {
    let manifest_path = verified_regular_file(manifest_path, "generated Loaf Cargo.toml")?;
    if !visiting.insert(manifest_path.clone()) {
        return Err(OvenLegacyCargoError::Plan(format!(
            "generated Loaf path dependency cycle reaches {}",
            manifest_path.display()
        )));
    }
    let package =
        manifest
            .get("package")
            .and_then(toml::Value::as_table)
            .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                field: "generated Loaf Cargo.toml",
                message: format!("{} has no [package] table", manifest_path.display()),
            })?;
    let name = package
        .get("name")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "generated Loaf Cargo.toml",
            message: format!("{} has no package name", manifest_path.display()),
        })?
        .to_string();
    let workspace = local_cargo_workspace_authority(&manifest_path, manifest)?;
    let (version, detached_compiler_lock_authority) =
        if let Some(version) = package.get("version").and_then(toml::Value::as_str) {
            (version.to_string(), false)
        } else if package
            .get("version")
            .and_then(toml::Value::as_table)
            .and_then(|version| version.get("workspace"))
            .and_then(toml::Value::as_bool)
            == Some(true)
        {
            match workspace.as_ref() {
                Some(workspace) => (inherited_workspace_package_version(workspace, &manifest_path)?, false),
                None => (detached_compiler_package_version(lock, &name, &manifest_path)?, true),
            }
        } else {
            return Err(OvenLegacyCargoError::InvalidInput {
                field: "generated Loaf Cargo.toml",
                message: format!("{} has no package version", manifest_path.display()),
            });
        };
    if !force_record && detached_compiler_lock_authority && lock_contains_package(lock, &name, &version, None) {
        visiting.remove(&manifest_path);
        return Ok(LockedLocalPackage {
            name,
            version,
            value: toml::Value::Table(toml::map::Map::new()),
        });
    }
    let mut dependencies = Vec::new();
    for dependency in manifest_dependency_entries(&manifest_path, manifest, workspace.as_ref())? {
        let dependency = locked_manifest_dependency(
            &dependency.declaration_root,
            &dependency.alias,
            &dependency.specification,
            lock,
            visiting,
            allow_project_registry_packages,
        )?;
        if let Some(dependency) = dependency {
            dependencies.push(dependency);
        }
    }
    dependencies.sort();
    dependencies.dedup();
    let mut value = toml::map::Map::new();
    value.insert("name".to_string(), toml::Value::String(name.clone()));
    value.insert("version".to_string(), toml::Value::String(version.clone()));
    if !dependencies.is_empty() {
        value.insert(
            "dependencies".to_string(),
            toml::Value::Array(dependencies.into_iter().map(toml::Value::String).collect()),
        );
    }
    if !force_record {
        let packages = lock
            .get_mut("package")
            .and_then(toml::Value::as_array_mut)
            .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                field: "compiler Cargo.lock",
                message: "must contain a package array".to_string(),
            })?;
        packages.retain(|package| {
            !(package.get("name").and_then(toml::Value::as_str) == Some(name.as_str())
                && package.get("version").and_then(toml::Value::as_str) == Some(version.as_str())
                && package.get("source").is_none())
        });
    }
    visiting.remove(&manifest_path);
    Ok(LockedLocalPackage {
        name,
        version,
        value: toml::Value::Table(value),
    })
}

/// Locate explicit Cargo workspace authority, otherwise the nearest containing workspace manifest.
pub fn local_cargo_workspace_authority(
    manifest_path: &Path,
    manifest: &toml::Value,
) -> Result<Option<LocalCargoWorkspaceAuthority>, OvenLegacyCargoError> {
    let manifest_root = manifest_path
        .parent()
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "generated Loaf Cargo.toml",
            message: format!("{} has no package directory", manifest_path.display()),
        })?;
    if let Some(explicit_workspace) = manifest
        .get("package")
        .and_then(toml::Value::as_table)
        .and_then(|package| package.get("workspace"))
    {
        let explicit_workspace = explicit_workspace
            .as_str()
            .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                field: "generated Loaf Cargo.toml",
                message: format!("{} has a non-string package.workspace path", manifest_path.display()),
            })?;
        return read_local_cargo_workspace_manifest(&manifest_root.join(explicit_workspace).join("Cargo.toml"))
            .map(Some);
    }
    if manifest.get("workspace").is_some() {
        if manifest.get("workspace").and_then(toml::Value::as_table).is_none() {
            return Err(OvenLegacyCargoError::InvalidInput {
                field: "generated Loaf Cargo.toml",
                message: format!("{} has a non-table [workspace] value", manifest_path.display()),
            });
        }
        return Ok(Some(LocalCargoWorkspaceAuthority {
            root: manifest_root.to_path_buf(),
            manifest_path: manifest_path.to_path_buf(),
            manifest: manifest.clone(),
        }));
    }
    for ancestor in manifest_root.ancestors().skip(1) {
        let candidate = ancestor.join("Cargo.toml");
        if !candidate.is_file() {
            continue;
        }
        let workspace = read_local_cargo_manifest(&candidate)?;
        match workspace.manifest.get("workspace") {
            Some(value) if value.is_table() => return Ok(Some(workspace)),
            Some(_) => {
                return Err(OvenLegacyCargoError::InvalidInput {
                    field: "generated Loaf workspace Cargo.toml",
                    message: format!(
                        "{} has a non-table [workspace] value",
                        workspace.manifest_path.display()
                    ),
                });
            }
            None => {}
        }
    }
    Ok(None)
}

/// Read and validate one explicitly selected Cargo workspace manifest.
pub fn read_local_cargo_workspace_manifest(
    manifest_path: &Path,
) -> Result<LocalCargoWorkspaceAuthority, OvenLegacyCargoError> {
    let workspace = read_local_cargo_manifest(manifest_path)?;
    if workspace
        .manifest
        .get("workspace")
        .and_then(toml::Value::as_table)
        .is_none()
    {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "generated Loaf workspace Cargo.toml",
            message: format!(
                "{} selected by package.workspace has no [workspace] table",
                workspace.manifest_path.display()
            ),
        });
    }
    Ok(workspace)
}

/// Parse one candidate workspace manifest while retaining its canonical path for diagnostics and dependency roots.
pub fn read_local_cargo_manifest(manifest_path: &Path) -> Result<LocalCargoWorkspaceAuthority, OvenLegacyCargoError> {
    let manifest_path = verified_regular_file(manifest_path, "generated Loaf workspace Cargo.toml")?;
    let bytes = fs::read(&manifest_path).map_err(|source| OvenLegacyCargoError::Io {
        path: manifest_path.clone(),
        source,
    })?;
    let manifest = toml::from_slice::<toml::Value>(&bytes).map_err(|error| OvenLegacyCargoError::InvalidInput {
        field: "generated Loaf workspace Cargo.toml",
        message: format!("{} is not valid TOML: {error}", manifest_path.display()),
    })?;
    let root = manifest_path
        .parent()
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "generated Loaf workspace Cargo.toml",
            message: format!("{} has no workspace directory", manifest_path.display()),
        })?
        .to_path_buf();
    Ok(LocalCargoWorkspaceAuthority {
        root,
        manifest_path,
        manifest,
    })
}

/// Resolve the lock identity field inherited from `[workspace.package]`.
pub fn inherited_workspace_package_version(
    workspace: &LocalCargoWorkspaceAuthority,
    package_manifest_path: &Path,
) -> Result<String, OvenLegacyCargoError> {
    workspace
        .manifest
        .get("workspace")
        .and_then(toml::Value::as_table)
        .and_then(|workspace| workspace.get("package"))
        .and_then(toml::Value::as_table)
        .and_then(|package| package.get("version"))
        .and_then(toml::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "generated Loaf workspace Cargo.toml",
            message: format!(
                "{} has no string [workspace.package].version inherited by {}",
                workspace.manifest_path.display(),
                package_manifest_path.display()
            ),
        })
}

/// Retain the checked compiler-lock fallback only for a package detached from its source workspace.
pub fn detached_compiler_package_version(
    lock: &toml::Value,
    name: &str,
    manifest_path: &Path,
) -> Result<String, OvenLegacyCargoError> {
    let candidates = lock_package_identities(lock)
        .into_iter()
        .filter(|(candidate_name, _, source)| candidate_name == name && source.is_none())
        .collect::<Vec<_>>();
    let [(_, version, _)] = candidates.as_slice() else {
        return Err(OvenLegacyCargoError::Plan(format!(
            "detached workspace package `{name}` at {} must select exactly one local version from the checked compiler lock, found {}",
            manifest_path.display(),
            candidates.len()
        )));
    };
    Ok(version.clone())
}

/// Collect effective dependency specifications from ordinary and target-specific Cargo manifest tables.
pub fn manifest_dependency_entries(
    manifest_path: &Path,
    manifest: &toml::Value,
    workspace: Option<&LocalCargoWorkspaceAuthority>,
) -> Result<Vec<EffectiveLocalCargoDependency>, OvenLegacyCargoError> {
    const SECTIONS: [&str; 3] = ["dependencies", "build-dependencies", "dev-dependencies"];
    let manifest_root = manifest_path
        .parent()
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "generated Loaf Cargo.toml",
            message: format!("{} has no package directory", manifest_path.display()),
        })?;
    let mut entries = Vec::new();
    for section in SECTIONS {
        if let Some(dependencies) = manifest.get(section).and_then(toml::Value::as_table) {
            collect_manifest_dependency_entries(manifest_path, manifest_root, dependencies, workspace, &mut entries)?;
        }
    }
    if let Some(targets) = manifest.get("target").and_then(toml::Value::as_table) {
        for target in targets.values().filter_map(toml::Value::as_table) {
            for section in SECTIONS {
                if let Some(dependencies) = target.get(section).and_then(toml::Value::as_table) {
                    collect_manifest_dependency_entries(
                        manifest_path,
                        manifest_root,
                        dependencies,
                        workspace,
                        &mut entries,
                    )?;
                }
            }
        }
    }
    Ok(entries)
}

/// Apply workspace inheritance to one dependency table without asking Cargo to rediscover its authority.
pub fn collect_manifest_dependency_entries(
    manifest_path: &Path,
    manifest_root: &Path,
    dependencies: &toml::map::Map<String, toml::Value>,
    workspace: Option<&LocalCargoWorkspaceAuthority>,
    entries: &mut Vec<EffectiveLocalCargoDependency>,
) -> Result<(), OvenLegacyCargoError> {
    for (alias, specification) in dependencies {
        let Some(member_table) = specification.as_table() else {
            entries.push(EffectiveLocalCargoDependency {
                alias: alias.clone(),
                specification: specification.clone(),
                declaration_root: manifest_root.to_path_buf(),
                inherited_workspace: false,
            });
            continue;
        };
        let Some(inherits) = member_table.get("workspace") else {
            entries.push(EffectiveLocalCargoDependency {
                alias: alias.clone(),
                specification: specification.clone(),
                declaration_root: manifest_root.to_path_buf(),
                inherited_workspace: false,
            });
            continue;
        };
        if inherits.as_bool() != Some(true) {
            return Err(OvenLegacyCargoError::InvalidInput {
                field: "generated Loaf Cargo.toml",
                message: format!(
                    "{} dependency `{alias}` must set workspace = true to inherit workspace authority",
                    manifest_path.display()
                ),
            });
        }
        let workspace = workspace.ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "generated Loaf Cargo.toml",
            message: format!(
                "{} dependency `{alias}` inherits [workspace.dependencies] but no containing Cargo workspace was found",
                manifest_path.display()
            ),
        })?;
        let inherited = workspace
            .manifest
            .get("workspace")
            .and_then(toml::Value::as_table)
            .and_then(|workspace| workspace.get("dependencies"))
            .and_then(toml::Value::as_table)
            .and_then(|dependencies| dependencies.get(alias))
            .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                field: "generated Loaf workspace Cargo.toml",
                message: format!(
                    "{} has no [workspace.dependencies].{alias} inherited by {}",
                    workspace.manifest_path.display(),
                    manifest_path.display()
                ),
            })?;
        entries.push(EffectiveLocalCargoDependency {
            alias: alias.clone(),
            specification: merged_workspace_dependency_specification(
                alias,
                inherited,
                member_table,
                &workspace.manifest_path,
                manifest_path,
            )?,
            declaration_root: workspace.root.clone(),
            inherited_workspace: true,
        });
    }
    Ok(())
}

/// Merge Cargo's member-local workspace dependency modifiers into the selected workspace declaration.
pub fn merged_workspace_dependency_specification(
    alias: &str,
    inherited: &toml::Value,
    member: &toml::map::Map<String, toml::Value>,
    workspace_manifest_path: &Path,
    member_manifest_path: &Path,
) -> Result<toml::Value, OvenLegacyCargoError> {
    let mut effective = match inherited {
        toml::Value::String(version) => {
            toml::map::Map::from_iter([("version".to_string(), toml::Value::String(version.clone()))])
        }
        toml::Value::Table(table) => table.clone(),
        _ => {
            return Err(OvenLegacyCargoError::InvalidInput {
                field: "generated Loaf workspace Cargo.toml",
                message: format!(
                    "{} [workspace.dependencies].{alias} must be a version string or dependency table",
                    workspace_manifest_path.display()
                ),
            });
        }
    };
    for (key, value) in member {
        match key.as_str() {
            "workspace" => {}
            "features" => merge_workspace_dependency_features(
                alias,
                &mut effective,
                value,
                workspace_manifest_path,
                member_manifest_path,
            )?,
            "optional" | "default-features" | "public" if value.is_bool() => {
                effective.insert(key.clone(), value.clone());
            }
            "optional" | "default-features" | "public" => {
                return Err(OvenLegacyCargoError::InvalidInput {
                    field: "generated Loaf Cargo.toml",
                    message: format!(
                        "{} dependency `{alias}` has a non-boolean `{key}` workspace modifier",
                        member_manifest_path.display()
                    ),
                });
            }
            _ => {
                return Err(OvenLegacyCargoError::InvalidInput {
                    field: "generated Loaf Cargo.toml",
                    message: format!(
                        "{} dependency `{alias}` cannot override workspace authority field `{key}`",
                        member_manifest_path.display()
                    ),
                });
            }
        }
    }
    if effective.get("workspace").is_some() {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "generated Loaf workspace Cargo.toml",
            message: format!(
                "{} [workspace.dependencies].{alias} cannot inherit from another workspace dependency",
                workspace_manifest_path.display()
            ),
        });
    }
    Ok(toml::Value::Table(effective))
}

/// Union the feature sets contributed by the workspace and its member declaration.
pub fn merge_workspace_dependency_features(
    alias: &str,
    effective: &mut toml::map::Map<String, toml::Value>,
    member_features: &toml::Value,
    workspace_manifest_path: &Path,
    member_manifest_path: &Path,
) -> Result<(), OvenLegacyCargoError> {
    let mut features = BTreeSet::new();
    if let Some(workspace_features) = effective.get("features") {
        let workspace_features = workspace_features
            .as_array()
            .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                field: "generated Loaf workspace Cargo.toml",
                message: format!(
                    "{} [workspace.dependencies].{alias}.features must be an array",
                    workspace_manifest_path.display()
                ),
            })?;
        for feature in workspace_features {
            let feature = feature.as_str().ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                field: "generated Loaf workspace Cargo.toml",
                message: format!(
                    "{} [workspace.dependencies].{alias}.features contains a non-string value",
                    workspace_manifest_path.display()
                ),
            })?;
            features.insert(feature.to_string());
        }
    }
    let member_features = member_features
        .as_array()
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "generated Loaf Cargo.toml",
            message: format!(
                "{} dependency `{alias}` has a non-array features workspace modifier",
                member_manifest_path.display()
            ),
        })?;
    for feature in member_features {
        let feature = feature.as_str().ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "generated Loaf Cargo.toml",
            message: format!(
                "{} dependency `{alias}` features contains a non-string value",
                member_manifest_path.display()
            ),
        })?;
        features.insert(feature.to_string());
    }
    effective.insert(
        "features".to_string(),
        toml::Value::Array(features.into_iter().map(toml::Value::String).collect()),
    );
    Ok(())
}

/// Resolve one generated-manifest dependency to an exact package identity already admitted by compiler authority.
pub fn locked_manifest_dependency(
    manifest_root: &Path,
    alias: &str,
    specification: &toml::Value,
    lock: &mut toml::Value,
    visiting: &mut BTreeSet<PathBuf>,
    allow_project_registry_packages: bool,
) -> Result<Option<String>, OvenLegacyCargoError> {
    if let Some(table) = specification.as_table()
        && let Some(path) = table.get("path").and_then(toml::Value::as_str)
    {
        let dependency_manifest = manifest_root.join(path).join("Cargo.toml");
        let dependency_text = regular_file_bytes(&dependency_manifest)?;
        let dependency_manifest_value =
            toml::from_str::<toml::Value>(std::str::from_utf8(&dependency_text).map_err(|error| {
                OvenLegacyCargoError::InvalidInput {
                    field: "generated Loaf path dependency Cargo.toml",
                    message: error.to_string(),
                }
            })?)
            .map_err(|error| OvenLegacyCargoError::InvalidInput {
                field: "generated Loaf path dependency Cargo.toml",
                message: error.to_string(),
            })?;
        let package = locked_local_package(
            &dependency_manifest,
            &dependency_manifest_value,
            lock,
            visiting,
            false,
            allow_project_registry_packages,
        )?;
        let declared_package = table.get("package").and_then(toml::Value::as_str).unwrap_or(alias);
        if declared_package != package.name {
            return Err(OvenLegacyCargoError::Plan(format!(
                "generated Loaf dependency `{alias}` declares package `{declared_package}` but {} names `{}`",
                dependency_manifest.display(),
                package.name
            )));
        }
        if !lock_contains_package(lock, &package.name, &package.version, None) {
            let packages = lock
                .get_mut("package")
                .and_then(toml::Value::as_array_mut)
                .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                    field: "compiler Cargo.lock",
                    message: "must contain a package array".to_string(),
                })?;
            packages.push(package.value);
        }
        let _ = lock_package_reference(lock, &package.name, &package.version, None)?;
        return Ok(Some(format!("{} {}", package.name, package.version)));
    }
    let package = specification
        .as_table()
        .and_then(|table| table.get("package"))
        .and_then(toml::Value::as_str)
        .unwrap_or(alias);
    let requirement = specification
        .as_str()
        .or_else(|| {
            specification
                .as_table()
                .and_then(|table| table.get("version"))
                .and_then(toml::Value::as_str)
        })
        .unwrap_or("*");
    let requirement = semver::VersionReq::parse(requirement).map_err(|error| OvenLegacyCargoError::InvalidInput {
        field: "generated Loaf Cargo.toml dependency version",
        message: format!("`{alias}` has invalid requirement: {error}"),
    })?;
    let candidates = lock_package_identities(lock)
        .into_iter()
        .filter(|(name, version, source)| {
            name == package
                && source.as_deref().is_some_and(|source| source.starts_with("registry+"))
                && semver::Version::parse(version).is_ok_and(|version| requirement.matches(&version))
        })
        .collect::<Vec<_>>();
    if candidates.is_empty() && allow_project_registry_packages {
        return Ok(None);
    }
    let [(name, version, source)] = candidates.as_slice() else {
        return Err(OvenLegacyCargoError::Plan(format!(
            "generated Loaf dependency `{alias}` ({package} {requirement}) must select exactly one package from the compiler lock, found {}",
            candidates.len()
        )));
    };
    lock_package_reference(lock, name, version, source.as_deref()).map(Some)
}
