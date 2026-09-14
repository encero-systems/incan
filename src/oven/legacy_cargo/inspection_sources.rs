//! Cargo metadata reads and the inspection sources an explicit bake derives from them.
//!
//! The publisher asks Cargo for metadata once, under the lock policy the bake selected, and turns the resolved
//! graph into the direct dependency packages, registry source dependencies and inspection sources that the rest
//! of the bake consumes. They are moved verbatim out of `legacy_cargo.rs`; the publisher that calls them lives
//! there.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::{
    CargoMetadata, Command, InspectionPackageScope, OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH, OvenLegacyCargoBaseLoaf,
    OvenLegacyCargoError, OvenLegacyCargoInspectionPackage, OvenLegacyCargoInspectionSource,
    OvenProjectRegistrySourceDependency, OvenRustcRegistrySourcePackage, ResolvedDirectDependency, canonical_tool_file,
    cargo_registry_checksums, clear_inherited_cargo_environment, digest_bytes, inspection_package_closure_ids,
    regular_file_bytes, stage_registry_source_directory, stage_release_cohort_project_lock, verified_regular_file,
};

/// Read Cargo's package identity metadata at the named publisher boundary without creating a target directory.
///
/// The unit graph remains the feature and edge authority. Metadata only decodes its opaque package IDs so Oven can
/// create a sealed third-party foundation manifest; no normal command calls this helper.
pub(crate) fn read_legacy_cargo_metadata(
    cargo: &Path,
    cargo_manifest: &Path,
    features: &[String],
) -> Result<CargoMetadata, OvenLegacyCargoError> {
    read_legacy_cargo_metadata_with_lock_policy(cargo, cargo_manifest, features, true)
}

/// Read publisher metadata with the caller's explicit Cargo.lock admission policy.
///
/// Compiler-owned publication always requires a pre-existing lock. The sole `false` caller is the explicit user
/// project bake, which may resolve and create its first Cargo.lock before its source and final compiler plans are
/// sealed. Every later publisher invocation is locked and offline against that newly sealed authority.
pub(crate) fn read_legacy_cargo_metadata_with_lock_policy(
    cargo: &Path,
    cargo_manifest: &Path,
    features: &[String],
    require_existing_lock: bool,
) -> Result<CargoMetadata, OvenLegacyCargoError> {
    read_legacy_cargo_metadata_for_platform(cargo, cargo_manifest, features, require_existing_lock, None)
}

/// Read publisher metadata with the caller's explicit Cargo.lock admission policy, optionally pruned to one target.
///
/// Without `filter_platform`, Cargo's resolve graph stays platform-agnostic: it retains every target's locked
/// closure, including a dependency that Cargo itself would never build for the exact target being sealed (for
/// example a Linux-only transitive dependency of a project-declared crate while baking for macOS). Passing the
/// receipt's exact target reproduces Cargo's own `--filter-platform` resolver decision, so a caller that needs to
/// know which locked packages a target's build actually requires gets Cargo's answer instead of a broader,
/// platform-agnostic guess.
pub(crate) fn read_legacy_cargo_metadata_for_platform(
    cargo: &Path,
    cargo_manifest: &Path,
    features: &[String],
    require_existing_lock: bool,
    filter_platform: Option<&str>,
) -> Result<CargoMetadata, OvenLegacyCargoError> {
    let cargo = canonical_tool_file(cargo, "cargo")?;
    let cargo_manifest = verified_regular_file(cargo_manifest, "Cargo manifest")?;
    let package_root = cargo_manifest
        .parent()
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "Cargo manifest",
            message: format!("{} has no package directory", cargo_manifest.display()),
        })?;
    let cargo_lock_exists = package_root.join("Cargo.lock").is_file();
    let mut command = Command::new(&cargo);
    command
        .current_dir(package_root)
        .arg("metadata")
        .arg("--manifest-path")
        .arg(&cargo_manifest)
        .args(["--format-version", "1"]);
    if require_existing_lock || cargo_lock_exists {
        command.arg("--offline");
    }
    if require_existing_lock {
        command.arg("--locked");
    }
    if let Some(target) = filter_platform {
        command.args(["--filter-platform", target]);
    }
    // The package records must be resolved through the same feature selection as the unit graph. Otherwise an
    // optional dependency may occur in the graph but be absent from this identity lookup, which leaves the named
    // foundation publisher unable to reproduce its declared third-party closure.
    if !features.is_empty() {
        command.arg("--features").arg(features.join(","));
    }
    clear_inherited_cargo_environment(&mut command);
    let output = command.output().map_err(|source| OvenLegacyCargoError::Io {
        path: cargo.clone(),
        source,
    })?;
    if !output.status.success() {
        return Err(OvenLegacyCargoError::CargoFailed {
            output: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    serde_json::from_slice(&output.stdout).map_err(|error| {
        OvenLegacyCargoError::Plan(format!(
            "the internal compatibility publisher emitted invalid compiler foundation metadata: {error}"
        ))
    })
}

/// Bind each declared direct dependency alias to the exact resolved Cargo package instance.
///
/// A direct-rustc invocation receives `--extern <alias>=<artifact>`, so choosing by library filename or package name
/// is unsound when a valid lock contains (for example) `substrait` 0.62 transitively and `substrait` 0.63 directly.
/// Cargo's root resolve edges preserve the alias and package ID relationship; retain it only long enough for the
/// named baker to select and seal the matching artifact.
pub(crate) fn resolve_direct_dependency_packages(
    metadata: &CargoMetadata,
    dependencies: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, ResolvedDirectDependency>, OvenLegacyCargoError> {
    if dependencies.is_empty() {
        return Ok(BTreeMap::new());
    }
    let resolve = metadata.resolve.as_ref().ok_or_else(|| {
        OvenLegacyCargoError::Plan(
            "locked Cargo metadata omitted the resolve graph required for direct-rustc dependency selection"
                .to_string(),
        )
    })?;
    let root_id = resolve.root.as_deref().ok_or_else(|| {
        OvenLegacyCargoError::Plan("locked Cargo metadata omitted the generated-project root package".to_string())
    })?;
    let packages = metadata
        .packages
        .iter()
        .map(|package| (package.id.as_str(), package))
        .collect::<BTreeMap<_, _>>();
    let root_package = packages.get(root_id).ok_or_else(|| {
        OvenLegacyCargoError::Plan(format!(
            "locked Cargo metadata root package `{root_id}` is absent from its package records"
        ))
    })?;
    let root = resolve.nodes.iter().find(|node| node.id == root_id).ok_or_else(|| {
        OvenLegacyCargoError::Plan(format!(
            "locked Cargo metadata root package `{root_id}` has no resolve node"
        ))
    })?;
    let normalize = |name: &str| name.replace('-', "_");
    let mut resolved = BTreeMap::new();
    for (alias, package) in dependencies {
        // The library-test publisher adds the root package's own library as an explicit input. Cargo does not model
        // that self-library as a root dependency edge, so it is the one principled exception to edge lookup.
        if normalize(alias) == normalize(&root_package.name) && package == &root_package.name {
            resolved.insert(
                alias.clone(),
                ResolvedDirectDependency {
                    package: package.clone(),
                    package_id: root_id.to_string(),
                },
            );
            continue;
        }
        let mut candidates = root
            .deps
            .iter()
            .filter(|edge| normalize(&edge.name) == normalize(alias))
            .filter(|edge| {
                packages
                    .get(edge.pkg.as_str())
                    .is_some_and(|candidate| candidate.name == *package)
            })
            .map(|edge| edge.pkg.clone())
            .collect::<Vec<_>>();
        candidates.sort();
        candidates.dedup();
        match candidates.as_slice() {
            [package_id] => {
                resolved.insert(
                    alias.clone(),
                    ResolvedDirectDependency {
                        package: package.clone(),
                        package_id: package_id.clone(),
                    },
                );
            }
            [] => {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "locked Cargo metadata has no root dependency edge for direct Rustc extern `{alias}` (package `{package}`)"
                )));
            }
            _ => {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "locked Cargo metadata resolves direct Rustc extern `{alias}` (package `{package}`) ambiguously: {}",
                    candidates.join(", ")
                )));
            }
        }
    }
    Ok(resolved)
}

/// Bind every direct registry alias in the generated root manifest to its exact sealed source identity.
///
/// This deliberately consumes the full declared root dependency map, not the smaller set of crates reachable from
/// one generated Rust source. Source inspection happens before that generated source exists, and two renamed aliases
/// can legitimately select distinct compatible versions of the same package. The explicit baker records Cargo's
/// exact root-edge decision so normal commands never have to repeat or approximate that resolution.
pub(crate) fn project_registry_source_dependencies(
    metadata: &CargoMetadata,
    dependencies: &BTreeMap<String, String>,
    registry_sources: &[OvenRustcRegistrySourcePackage],
) -> Result<Vec<OvenProjectRegistrySourceDependency>, OvenLegacyCargoError> {
    if dependencies.is_empty() {
        return Ok(Vec::new());
    }
    let resolve = metadata.resolve.as_ref().ok_or_else(|| {
        OvenLegacyCargoError::Plan(
            "locked Cargo metadata omitted the resolve graph required for project source authority".to_string(),
        )
    })?;
    let root_id = resolve.root.as_deref().ok_or_else(|| {
        OvenLegacyCargoError::Plan("locked Cargo metadata omitted the generated-project root package".to_string())
    })?;
    let packages = metadata
        .packages
        .iter()
        .map(|package| (package.id.as_str(), package))
        .collect::<BTreeMap<_, _>>();
    let root = resolve.nodes.iter().find(|node| node.id == root_id).ok_or_else(|| {
        OvenLegacyCargoError::Plan(format!(
            "locked Cargo metadata root package `{root_id}` has no resolve node"
        ))
    })?;
    let normalize = |name: &str| name.replace('-', "_");
    let mut selected = Vec::new();
    for (alias, declared_package) in dependencies {
        let mut candidates = root
            .deps
            .iter()
            .filter(|edge| normalize(&edge.name) == normalize(alias))
            .filter_map(|edge| packages.get(edge.pkg.as_str()).copied())
            .filter(|package| package.name == *declared_package)
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| left.id.cmp(&right.id));
        candidates.dedup_by(|left, right| left.id == right.id);
        let package = match candidates.as_slice() {
            [package] => *package,
            [] => {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "locked Cargo metadata has no root dependency edge for `{alias}` (package `{declared_package}`)"
                )));
            }
            _ => {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "locked Cargo metadata resolves root dependency `{alias}` (package `{declared_package}`) ambiguously"
                )));
            }
        };
        let Some(registry) = package.source.as_deref() else {
            continue;
        };
        if !registry.starts_with("registry+") {
            return Err(OvenLegacyCargoError::Plan(format!(
                "root dependency `{alias}` selected unsupported source `{registry}`"
            )));
        }
        let matches = registry_sources
            .iter()
            .filter(|source| {
                source.package == package.name
                    && source.version == package.version
                    && source.source.registry == registry
            })
            .collect::<Vec<_>>();
        let [source] = matches.as_slice() else {
            return Err(OvenLegacyCargoError::Plan(format!(
                "root registry dependency `{alias}` (package `{}` {} from `{registry}`) has {} exact sealed source records",
                package.name,
                package.version,
                matches.len()
            )));
        };
        selected.push(OvenProjectRegistrySourceDependency {
            alias: alias.clone(),
            package: package.name.clone(),
            version: package.version.clone(),
            registry: registry.to_string(),
            checksum: source.source.checksum.clone(),
        });
    }
    selected.sort_by(|left, right| left.alias.cmp(&right.alias));
    if selected.windows(2).any(|window| window[0].alias == window[1].alias) {
        return Err(OvenLegacyCargoError::Plan(
            "generated project declares duplicate registry dependency aliases".to_string(),
        ));
    }
    Ok(selected)
}

/// Resolve and digest the exact registry sources available to a child of the explicit Loaf baker.
///
/// This is deliberately not a normal-command resolver. It runs locked, offline Cargo metadata at the already named
/// `legacy_cargo` boundary, joins those package IDs to the publisher lock checksums, and returns a typed authority
/// that a fixture child can consume without launching Cargo or searching an ambient Cargo home.
pub fn legacy_cargo_inspection_sources(
    cargo: &Path,
    cargo_manifest: &Path,
    features: &[String],
    inspection_packages: &[OvenLegacyCargoInspectionPackage],
    staging: &Path,
) -> Result<Vec<OvenLegacyCargoInspectionSource>, OvenLegacyCargoError> {
    let metadata = read_legacy_cargo_metadata(cargo, cargo_manifest, features)?;
    let package_root = cargo_manifest
        .parent()
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "Cargo manifest",
            message: format!("{} has no package directory", cargo_manifest.display()),
        })?;
    let lock = regular_file_bytes(&package_root.join("Cargo.lock"))?;
    legacy_cargo_inspection_sources_from_metadata(
        &metadata,
        &lock,
        inspection_packages,
        InspectionPackageScope::ResolvedGraph,
        staging,
    )
}

/// Resolve inspection sources for one explicit user-requested project bake.
///
/// Unlike compiler-owned Loaf publication, a conventional Incan project may have only the semantic `oven.lock` and
/// no pre-existing Cargo.lock. This function therefore permits Cargo to create its first lock while remaining offline
/// and inside the named `incan oven bake` transaction. Its caller immediately seals the copied, digested sources and
/// the final direct-Rustc publisher records its independently generated lock. Normal build, run, and test cannot
/// call this helper.
pub fn explicit_project_bake_inspection_sources(
    cargo: &Path,
    cargo_manifest: &Path,
    features: &[String],
    inspection_packages: &[OvenLegacyCargoInspectionPackage],
    staging: &Path,
    release_registry_lock: Option<&Path>,
) -> Result<Vec<OvenLegacyCargoInspectionSource>, OvenLegacyCargoError> {
    let package_root = cargo_manifest
        .parent()
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "Cargo manifest",
            message: format!("{} has no package directory", cargo_manifest.display()),
        })?;
    let metadata = match release_registry_lock {
        Some(release_registry_lock) => {
            stage_release_cohort_project_lock(cargo, package_root, release_registry_lock, features)?
        }
        None => read_legacy_cargo_metadata_with_lock_policy(cargo, cargo_manifest, features, false)?,
    };
    let lock = regular_file_bytes(&package_root.join("Cargo.lock"))?;
    legacy_cargo_inspection_sources_from_metadata(
        &metadata,
        &lock,
        inspection_packages,
        InspectionPackageScope::CompleteResolvedGraph,
        staging,
    )
}

/// Return the digest-verified registry lock owned by one selected release cohort.
pub(crate) fn verified_release_cohort_registry_lock(
    base: &OvenLegacyCargoBaseLoaf<'_>,
) -> Result<PathBuf, OvenLegacyCargoError> {
    base.artifacts
        .validate_shape(&base.artifacts.intent)
        .map_err(|error| OvenLegacyCargoError::Plan(format!("selected release-cohort plan is invalid: {error}")))?;
    let matches = base
        .artifacts
        .supporting_artifacts
        .iter()
        .filter(|artifact| artifact.relative_path == OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH)
        .collect::<Vec<_>>();
    let [declared] = matches.as_slice() else {
        return Err(OvenLegacyCargoError::Plan(format!(
            "selected release Loaf must declare exactly one `{OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH}` artifact, found {}",
            matches.len()
        )));
    };
    let path = base.artifact_root.join(OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH);
    let bytes = regular_file_bytes(&path)?;
    let actual = digest_bytes(&bytes);
    if actual != declared.digest {
        return Err(OvenLegacyCargoError::Plan(format!(
            "selected release Loaf registry lock digest mismatch: expected {}, got {actual}",
            declared.digest
        )));
    }
    Ok(path)
}

/// Resolve and stage every registry source in one locked compiler feature graph.
///
/// The compiler suite synthesizes several workspace locks during its tests. Sealing the canonical resolved graph
/// avoids a second dependency inventory that can drift from those locks, while still keeping this resolution inside
/// the named `legacy_cargo` baker.
pub fn legacy_cargo_resolved_registry_sources(
    cargo: &Path,
    cargo_manifest: &Path,
    features: &[String],
    staging: &Path,
) -> Result<Vec<OvenLegacyCargoInspectionSource>, OvenLegacyCargoError> {
    let metadata = read_legacy_cargo_metadata(cargo, cargo_manifest, features)?;
    let package_root = cargo_manifest
        .parent()
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "Cargo manifest",
            message: format!("{} has no package directory", cargo_manifest.display()),
        })?;
    let lock = regular_file_bytes(&package_root.join("Cargo.lock"))?;
    legacy_cargo_inspection_sources_from_metadata(
        &metadata,
        &lock,
        &[],
        InspectionPackageScope::CompleteResolvedGraph,
        staging,
    )
}

/// Resolve one checked inspection surface from metadata already produced by the named publisher.
pub(crate) fn legacy_cargo_inspection_sources_from_metadata(
    metadata: &CargoMetadata,
    cargo_lock: &[u8],
    inspection_packages: &[OvenLegacyCargoInspectionPackage],
    scope: InspectionPackageScope,
    staging: &Path,
) -> Result<Vec<OvenLegacyCargoInspectionSource>, OvenLegacyCargoError> {
    let selected_package_ids = inspection_package_closure_ids(metadata, inspection_packages, scope)?;
    let checksums = cargo_registry_checksums(cargo_lock)?;
    let resolved_features = metadata
        .resolve
        .iter()
        .flat_map(|resolve| &resolve.nodes)
        .map(|node| (node.id.as_str(), node.features.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut sources = Vec::new();
    for package in &metadata.packages {
        if !selected_package_ids.contains(&package.id) {
            continue;
        }
        let Some(registry) = package
            .source
            .as_deref()
            .filter(|source| source.starts_with("registry+"))
        else {
            continue;
        };
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
                message: format!("{} has no package directory", package.manifest_path.display()),
            })?;
        let mut package_features = resolved_features.get(package.id.as_str()).cloned().unwrap_or_default();
        package_features.sort();
        package_features.dedup();
        let (source_root, source_digest) = stage_registry_source_directory(
            staging,
            &package.name,
            &package.version,
            registry,
            &checksum,
            source_root,
        )?;
        sources.push(OvenLegacyCargoInspectionSource {
            package: package.name.clone(),
            version: package.version.clone(),
            registry: registry.to_string(),
            checksum,
            features: package_features,
            source_root,
            source_digest,
        });
    }
    sources.sort_by(|left, right| {
        (&left.package, &left.version, &left.registry, &left.checksum).cmp(&(
            &right.package,
            &right.version,
            &right.registry,
            &right.checksum,
        ))
    });
    Ok(fold_repeated_inspection_sources(sources))
}

/// Fold registry sources that name the same staged tree, so each one contributes its files exactly once.
///
/// One registry package can be resolved more than once -- the same crate reached as a normal and a build
/// dependency, or under two feature resolutions -- and every one of those resolutions stages the same source tree
/// under the same content identity, because that identity is a digest of registry, package, version and checksum.
/// Each surviving entry then contributes that tree's files again, and the publisher refuses its own manifest with
/// "declares one relative artifact path more than once".
///
/// Features are unioned rather than dropped: the repeats are the same unit seen through different resolutions, and
/// the compilation observes every feature any of them asked for. The input must already be sorted on the same four
/// fields, which is what makes an adjacent comparison sufficient.
pub(crate) fn fold_repeated_inspection_sources(
    sources: Vec<OvenLegacyCargoInspectionSource>,
) -> Vec<OvenLegacyCargoInspectionSource> {
    let mut folded: Vec<OvenLegacyCargoInspectionSource> = Vec::with_capacity(sources.len());
    for source in sources {
        match folded.last_mut() {
            Some(previous)
                if previous.package == source.package
                    && previous.version == source.version
                    && previous.registry == source.registry
                    && previous.checksum == source.checksum =>
            {
                previous.features.extend(source.features);
                previous.features.sort();
                previous.features.dedup();
            }
            _ => folded.push(source),
        }
    }
    folded
}
