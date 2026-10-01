//! Composing runtime and registry foundations into a direct-rustc artifact manifest.

use super::{
    BTreeMap, BTreeSet, OVEN_COMPILER_RUNTIME_CRATE_PREFIX, OvenRustcArtifactExtern, OvenRustcArtifactManifest,
    OvenRustcError, OvenRustcRegistryLeaf, OvenRustcSupportingArtifact, Path, artifact_is_below_search_path,
};

/// Return the compiler-owned crate name encoded by one Cargo artifact filename.
pub(super) fn compiler_runtime_name_from_artifact_path(relative_path: &str) -> Option<&str> {
    let filename = Path::new(relative_path).file_name()?.to_str()?;
    let crate_and_digest = filename.strip_prefix("lib")?.split_once('-')?.0;
    crate_and_digest
        .strip_prefix(OVEN_COMPILER_RUNTIME_CRATE_PREFIX)
        .map(|_| crate_and_digest)
}

/// Index the unique host/target runtime family declared by an immutable standard-library Loaf.
///
/// Vocabulary auxiliary targets are deliberately excluded: the same `incan_vocab` crate can occur for the host and
/// Wasm target, while this index owns only the normal generated-program ABI family.
pub(super) fn compiler_runtime_artifacts_by_name(
    base: &OvenRustcArtifactManifest,
) -> Result<BTreeMap<String, OvenRustcSupportingArtifact>, OvenRustcError> {
    let main_search_paths = base
        .dependency_search_paths
        .iter()
        .chain(&base.native_search_paths)
        .map(String::as_str)
        .collect::<Vec<_>>();
    let artifacts = base
        .externs
        .iter()
        .map(|artifact| OvenRustcSupportingArtifact {
            relative_path: artifact.relative_path.clone(),
            digest: artifact.digest.clone(),
        })
        .chain(base.supporting_artifacts.iter().filter_map(|artifact| {
            main_search_paths
                .iter()
                .any(|search_path| artifact_is_below_search_path(&artifact.relative_path, search_path))
                .then_some(artifact.clone())
        }));
    let mut indexed = BTreeMap::new();
    for artifact in artifacts {
        // A `.rmeta` sidecar accompanies a split-metadata rlib (Rust 1.98+) inside the same directory. It is a
        // required companion of the linkable artifact, never a second runtime candidate, so it must not trip the
        // one-artifact-per-crate refusal below or ever be selected as an extern replacement.
        if artifact.relative_path.ends_with(".rmeta") {
            continue;
        }
        let Some(crate_name) = compiler_runtime_name_from_artifact_path(&artifact.relative_path).map(str::to_string)
        else {
            continue;
        };
        if indexed.insert(crate_name.clone(), artifact).is_some() {
            return Err(OvenRustcError::InvalidInput {
                field: "project extension base",
                message: format!("declares multiple compiler runtime artifacts for `{crate_name}`"),
            });
        }
    }
    Ok(indexed)
}

/// Drop `.rmeta` sidecars whose paired rlib is no longer declared by the composed manifest.
///
/// Cohort and registry-leaf replacement swap an extension's compiled rlibs for the selected release family's
/// artifacts. A sidecar names its exact sibling (`libX-<hash>.rmeta` beside `libX-<hash>.rlib`), so once that
/// sibling is replaced the sidecar describes a discarded compilation: materializing it creates a metadata-only
/// crate candidate that rustc can select and then reject with "required to be available in rlib format". A sidecar
/// therefore lives and dies with its declared sibling.
pub(super) fn discard_orphaned_metadata_sidecars(manifest: &mut OvenRustcArtifactManifest) {
    let declared_linkable_stems = manifest
        .externs
        .iter()
        .map(|artifact| artifact.relative_path.as_str())
        .chain(
            manifest
                .supporting_artifacts
                .iter()
                .map(|artifact| artifact.relative_path.as_str()),
        )
        .chain(
            manifest
                .registry_leaves
                .iter()
                .map(|leaf| leaf.artifact.relative_path.as_str()),
        )
        .chain(
            manifest
                .vocab_auxiliary_targets
                .iter()
                .flat_map(|auxiliary| auxiliary.externs.iter().map(|artifact| artifact.relative_path.as_str())),
        )
        .filter_map(|path| path.strip_suffix(".rlib").map(str::to_string))
        .collect::<BTreeSet<_>>();
    for artifact in &manifest.supporting_artifacts {
        if artifact
            .relative_path
            .strip_suffix(".rmeta")
            .is_some_and(|stem| !declared_linkable_stems.contains(stem))
        {
            for closure in manifest.entrypoint_dependency_search_paths.values_mut() {
                closure.replace_artifact(artifact, None);
            }
        }
    }
    manifest.supporting_artifacts.retain(|artifact| {
        let Some(stem) = artifact.relative_path.strip_suffix(".rmeta") else {
            return true;
        };
        declared_linkable_stems.contains(stem)
    });
}

/// Directory that holds a project extension's retained artifacts whose filenames collide with the selected release
/// base's execution closure.
///
/// A salted extension unit carries a StableCrateId distinct from the sealed base's twin, so both copies of a
/// shared interior unit legally coexist in one crate graph — but Cargo still names them identically, because the
/// rustc-level `-C metadata` salt never enters Cargo's extra-filename hash. Artifact records are keyed by relative
/// path, so the extension's copy moves into this sibling of its `deps` directory; rustc selects each dependent's
/// copy by recorded hash, and the filename — the linkage identity — never changes.
pub const OVEN_EXTENSION_REROOT_DIR: &str = "extension-deps";

/// Return the `extension-deps` sibling path for an artifact that lives directly in a `deps` directory.
///
/// Returns `None` for artifacts outside a `deps` directory (registry sources, build-script `out/` staging, native
/// libraries): those have no re-rooted home, so a digest collision there remains a genuine incompatibility.
pub(super) fn rerooted_extension_artifact_path(relative_path: &str) -> Option<String> {
    let path = Path::new(relative_path);
    let file_name = path.file_name()?;
    let parent = path.parent()?;
    if parent.file_name()? != "deps" {
        return None;
    }
    let rerooted = match parent.parent() {
        Some(grandparent) => grandparent.join(OVEN_EXTENSION_REROOT_DIR).join(file_name),
        None => Path::new(OVEN_EXTENSION_REROOT_DIR).join(file_name),
    };
    Some(rerooted.to_string_lossy().into_owned())
}

/// Return the original `deps` staging location for a re-rooted extension artifact, or `None` when the path is not
/// re-rooted.
///
/// [`OvenRustcArtifactManifest::materialized_artifacts`] resolves every artifact's source as the staging root joined
/// with its recorded relative path, so the publisher stages a re-rooted artifact by linking the built file from this
/// returned `deps` location into its `extension-deps` home before atomic copying.
pub fn rerooted_artifact_staging_source(relative_path: &str) -> Option<String> {
    let path = Path::new(relative_path);
    let file_name = path.file_name()?;
    let parent = path.parent()?;
    if parent.file_name()? != OVEN_EXTENSION_REROOT_DIR {
        return None;
    }
    let source = match parent.parent() {
        Some(grandparent) => grandparent.join("deps").join(file_name),
        None => Path::new("deps").join(file_name),
    };
    Some(source.to_string_lossy().into_owned())
}

/// Return the metadata-sidecar partner of a compiled artifact path: `libX-<hash>.rlib` pairs with the sibling
/// `libX-<hash>.rmeta` and vice versa.
///
/// A split-metadata rlib (Rust 1.98+) and its sidecar are one compilation: whenever cohort composition moves one of
/// them, the partner must move with it, or the stranded half becomes a metadata-only or metadata-less candidate that
/// rustc selects and then rejects.
pub(crate) fn metadata_sidecar_pair_path(relative_path: &str) -> Option<String> {
    if let Some(stem) = relative_path.strip_suffix(".rlib") {
        return Some(format!("{stem}.rmeta"));
    }
    relative_path.strip_suffix(".rmeta").map(|stem| format!("{stem}.rlib"))
}

/// Index the unique `.rmeta` sidecar declared per compiler-runtime crate by an immutable standard-library Loaf.
///
/// Since Rust 1.98 a runtime rlib may carry only a metadata stub, with the crate's real metadata in the sibling
/// `.rmeta` retained beside it. This index lets cohort replacement rewrite an extension's sidecar to the release
/// family's sidecar; crates whose release rlib embeds metadata simply have no entry here.
pub(super) fn compiler_runtime_sidecars_by_name(
    base: &OvenRustcArtifactManifest,
) -> Result<BTreeMap<String, OvenRustcSupportingArtifact>, OvenRustcError> {
    let main_search_paths = base
        .dependency_search_paths
        .iter()
        .chain(&base.native_search_paths)
        .map(String::as_str)
        .collect::<Vec<_>>();
    let mut indexed = BTreeMap::new();
    for artifact in base.supporting_artifacts.iter().filter(|artifact| {
        artifact.relative_path.ends_with(".rmeta")
            && main_search_paths
                .iter()
                .any(|search_path| artifact_is_below_search_path(&artifact.relative_path, search_path))
    }) {
        let Some(crate_name) = compiler_runtime_name_from_artifact_path(&artifact.relative_path).map(str::to_string)
        else {
            continue;
        };
        if indexed.insert(crate_name.clone(), artifact.clone()).is_some() {
            return Err(OvenRustcError::InvalidInput {
                field: "project extension base",
                message: format!("declares multiple compiler runtime metadata sidecars for `{crate_name}`"),
            });
        }
    }
    Ok(indexed)
}

/// Replace one compiler-owned direct extern with the exact selected release artifact.
pub(super) fn replace_compiler_runtime_extern(
    artifact: &mut OvenRustcArtifactExtern,
    base_artifacts: &BTreeMap<String, OvenRustcSupportingArtifact>,
) -> Result<(), OvenRustcError> {
    let Some(base_artifact) = base_artifacts.get(&artifact.crate_name) else {
        // The `incan_` prefix alone does not confer compiler ownership. A caller crate such as `incan_partner`
        // remains project-owned unless the selected release Loaf declares that exact runtime crate identity.
        return Ok(());
    };
    artifact.relative_path = base_artifact.relative_path.clone();
    artifact.digest = base_artifact.digest.clone();
    Ok(())
}

/// Replace one compiler-owned supporting artifact with the exact selected release artifact.
///
/// A `.rmeta` sidecar follows its replaced rlib rather than the linkable index: when the selected release family
/// carries its own sidecar for the crate, the extension's sidecar is rewritten to it; when the release rlib embeds
/// its metadata, the extension's now-orphaned sidecar is dropped (`None`). Rewriting a sidecar to the rlib path
/// would double-declare the rlib in the composed manifest.
pub(super) fn replace_compiler_runtime_supporting_artifact(
    artifact: OvenRustcSupportingArtifact,
    base_artifacts: &BTreeMap<String, OvenRustcSupportingArtifact>,
    base_sidecars: &BTreeMap<String, OvenRustcSupportingArtifact>,
) -> Option<OvenRustcSupportingArtifact> {
    let Some(crate_name) = compiler_runtime_name_from_artifact_path(&artifact.relative_path) else {
        return Some(artifact);
    };
    if artifact.relative_path.ends_with(".rmeta") {
        if !base_artifacts.contains_key(crate_name) {
            // Preserve a caller-owned `incan_*` sidecar that is absent from the selected release family.
            return Some(artifact);
        }
        return base_sidecars.get(crate_name).cloned();
    }
    let Some(base_artifact) = base_artifacts.get(crate_name) else {
        // Preserve a caller-owned `incan_*` artifact that is absent from the selected release family.
        return Some(artifact);
    };
    Some(base_artifact.clone())
}

/// Return the base artifacts required to execute its main and vocabulary closures.
///
/// A release Loaf can also retain registry source authority and provenance used by its own publisher. Those files
/// do not become inputs to every project extension. The project plan keeps its own locked registry catalog, while
/// this projection carries only files reachable through base search paths. Vocabulary root externs remain declared
/// by their auxiliary roles and are therefore excluded from the supporting list to avoid duplicate path ownership.
pub(super) fn compiler_runtime_execution_support(
    base: &OvenRustcArtifactManifest,
) -> Result<Vec<OvenRustcSupportingArtifact>, OvenRustcError> {
    let auxiliary_extern_paths = base
        .vocab_auxiliary_targets
        .iter()
        .flat_map(|auxiliary| auxiliary.externs.iter().map(|artifact| artifact.relative_path.as_str()))
        .collect::<BTreeSet<_>>();
    let search_paths = base
        .dependency_search_paths
        .iter()
        .chain(&base.native_search_paths)
        .chain(
            base.vocab_auxiliary_targets
                .iter()
                .flat_map(|auxiliary| auxiliary.dependency_search_paths.iter()),
        )
        .map(String::as_str)
        .collect::<Vec<_>>();
    Ok(base
        .composition_artifacts()?
        .into_iter()
        .filter(|artifact| {
            !auxiliary_extern_paths.contains(artifact.relative_path.as_str())
                && search_paths
                    .iter()
                    .any(|search_path| artifact_is_below_search_path(&artifact.relative_path, search_path))
        })
        .collect())
}

/// Verify that every registry package shared with the release uses the release's exact semantic coordinate.
pub(super) fn validate_release_registry_cohort(
    project: &OvenRustcArtifactManifest,
    base: &OvenRustcArtifactManifest,
) -> Result<(), OvenRustcError> {
    for package in &project.registry_sources {
        let candidates = base
            .registry_sources
            .iter()
            .filter(|candidate| {
                candidate.package == package.package
                    && candidate.version == package.version
                    && candidate.source.registry == package.source.registry
            })
            .collect::<Vec<_>>();
        if candidates.is_empty() {
            continue;
        }
        if !candidates.iter().any(|candidate| candidate.source == package.source) {
            let release_candidates = base
                .registry_sources
                .iter()
                .filter(|candidate| {
                    candidate.package == package.package
                        && candidate.version == package.version
                        && candidate.source.registry == package.source.registry
                })
                .map(|candidate| {
                    format!(
                        "{} features={:?} checksum={}",
                        candidate.version, candidate.features, candidate.source.checksum
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            return Err(OvenRustcError::InvalidInput {
                field: "project extension release cohort",
                message: format!(
                    "registry package `{}` {} checksum={} does not match the selected release source identity ({release_candidates})",
                    package.package, package.version, package.source.checksum
                ),
            });
        }
    }
    Ok(())
}

/// Return whether two registry leaves differ only in their independently published artifact bytes.
pub(super) fn same_registry_leaf_semantics(left: &OvenRustcRegistryLeaf, right: &OvenRustcRegistryLeaf) -> bool {
    left.package == right.package
        && left.version == right.version
        && left.crate_name == right.crate_name
        && left.domain == right.domain
        && left.crate_kind == right.crate_kind
        && left.source == right.source
        && left.features == right.features
}

/// Return whether a project registry leaf may be replaced by its matching release-base counterpart.
///
/// Matching package, version, source checksum, and declared features is necessary but not sufficient: Cargo's unit
/// identity — the `-<hash>` extra-filename suffix — also folds in resolved transitive features, profile details,
/// and the identities of the unit's own dependencies, so the same declared coordinates can legitimately compile to
/// a different crate identity inside a different closure (Bevy's `rand_core` vs the release closure's, #1227).
///
/// Two substitution regimes follow from who consumes the leaf, and the caller passes that judgment as
/// `allow_cross_identity`. A leaf consumed only by recompiled surfaces — the generated root's direct dependencies,
/// or any leaf in a plan with no prebuilt project crates — may swap onto the release copy even when identities
/// differ: that swap is exactly what unifies the root's trait identities (`serde::Serialize`) with the sealed
/// standard library. A leaf that prebuilt extension crates recorded by exact identity hash must not: substituting a
/// different identity removes the only artifact those records can resolve and the sealed closure stops loading.
///
/// In that conservative regime a matching filename is still not enough. The `-<hash>` suffix summarizes Cargo's
/// declared unit inputs, but the strict version hash recorded by consumers also reflects the concrete build
/// environment — a release base prebuilt on another machine publishes the same filename with a different SVH, and a
/// retained consumer compiled here can only load the local build (#1227, published-toolchain lane). Substitution is
/// therefore restricted to bit-identical artifacts — same filename and same content digest — making it a pure
/// byte-canonicalization onto the release's published copy and nothing more.
///
/// A build-script artifact is excluded in both regimes: `build.rs` can probe the ambient build environment and
/// diverge despite identical recorded inputs. It is staged under `build/<package>/<identity>/out/`, unlike an
/// ordinary crate's `deps/` output, so restrict substitution to leaves on both sides that are not
/// build-script-shaped.
pub(super) fn registry_leaf_substitution_is_safe(
    project_leaf: &OvenRustcRegistryLeaf,
    release_leaf: &OvenRustcRegistryLeaf,
    allow_cross_identity: bool,
) -> bool {
    let is_build_script_shaped = |relative_path: &str| {
        Path::new(relative_path)
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == "out")
    };
    if is_build_script_shaped(&project_leaf.artifact.relative_path)
        || is_build_script_shaped(&release_leaf.artifact.relative_path)
    {
        return false;
    }
    if allow_cross_identity {
        return true;
    }
    let project_file_name = Path::new(&project_leaf.artifact.relative_path).file_name();
    let release_file_name = Path::new(&release_leaf.artifact.relative_path).file_name();
    project_file_name.is_some()
        && project_file_name == release_file_name
        && project_leaf.artifact.digest == release_leaf.artifact.digest
}

/// Replace project source-catalog facts with the exact selected release record for each shared coordinate.
pub(super) fn canonicalize_release_registry_sources(
    project: &mut OvenRustcArtifactManifest,
    base: &OvenRustcArtifactManifest,
) -> Result<(), OvenRustcError> {
    for package in &mut project.registry_sources {
        let project_features = package.features.iter().collect::<BTreeSet<_>>();
        let candidates = base
            .registry_sources
            .iter()
            .filter(|candidate| {
                candidate.package == package.package
                    && candidate.version == package.version
                    && candidate.source == package.source
                    && project_features.is_subset(&candidate.features.iter().collect())
            })
            .collect::<Vec<_>>();
        match candidates.as_slice() {
            [] => {}
            [release] => *package = (*release).clone(),
            _ => {
                return Err(OvenRustcError::InvalidInput {
                    field: "project extension release cohort",
                    message: format!(
                        "registry source `{}` {} matches more than one selected release record",
                        package.package, package.version
                    ),
                });
            }
        }
    }
    Ok(())
}

/// Replace a declared project artifact by its exact release-cohort copy.
pub(super) fn replace_declared_release_artifact(
    manifest: &mut OvenRustcArtifactManifest,
    project_path: &str,
    release: &OvenRustcArtifactExtern,
) {
    let originals = manifest
        .externs
        .iter()
        .filter(|artifact| artifact.relative_path == project_path)
        .map(|artifact| OvenRustcSupportingArtifact {
            relative_path: artifact.relative_path.clone(),
            digest: artifact.digest.clone(),
        })
        .chain(
            manifest
                .supporting_artifacts
                .iter()
                .filter(|artifact| artifact.relative_path == project_path)
                .cloned(),
        )
        .collect::<Vec<_>>();
    if let Some(artifact) = manifest
        .externs
        .iter_mut()
        .find(|artifact| artifact.relative_path == project_path)
    {
        artifact.relative_path = release.relative_path.clone();
        artifact.digest = release.digest.clone();
    }
    if let Some(artifact) = manifest
        .supporting_artifacts
        .iter_mut()
        .find(|artifact| artifact.relative_path == project_path)
    {
        artifact.relative_path = release.relative_path.clone();
        artifact.digest = release.digest.clone();
    }
    for original in originals {
        for closure in manifest.entrypoint_dependency_search_paths.values_mut() {
            closure.replace_artifact(
                &original,
                Some(&OvenRustcSupportingArtifact {
                    relative_path: release.relative_path.clone(),
                    digest: release.digest.clone(),
                }),
            );
        }
    }
}
