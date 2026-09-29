//! The registry leaf authority: which sealed Loaf root answers for one registry package requirement.
//!
//! A caller-visible registry dependency is served only from the catalog the selected plan sealed, never from an
//! aggregate Cargo cache. These helpers resolve and validate that leaf against the plan's search paths.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::{
    DependencySpec, OvenRustcArtifactPlan, OvenRustcError, OvenRustcRegistryLeaf, Version, VersionReq,
    canonical_directory, digest_bytes, fs, normalized_relative_path, verified_regular_file,
};

/// One registry leaf and the immutable Loaf root that seals its relative artifact path.
#[derive(Debug, Clone)]
pub struct OvenRegistryLeafAuthorityEntry {
    artifact_root: PathBuf,
    pub leaf: OvenRustcRegistryLeaf,
    /// Already materialized immutable directories allowed to satisfy this leaf's transitive Rust metadata.
    ///
    /// A direct `--extern` names the immediate leaf, but Rustc still has to load the exact dependency artifacts
    /// recorded in that leaf's metadata. These directories come only from the same verified artifact plan that
    /// sealed the catalog; this is not an ambient Cargo search path.
    dependency_search_paths: Vec<PathBuf>,
}

/// Immutable registry-leaf authority supplied by receipt-compatible Loafs.
///
/// A caller receives this only from compiler Loafs that independently authorize the same receipt or the suite
/// scheduler's leased copies of those Loafs. Each entry retains its own root, so a narrow code plan may use a
/// registry leaf sealed by another compatible unit without treating either catalog as a Cargo-home, registry-index,
/// or download fallback.
#[derive(Debug, Clone)]
pub struct OvenRegistryLeafAuthority {
    entries: Vec<OvenRegistryLeafAuthorityEntry>,
}

impl OvenRegistryLeafAuthority {
    #[cfg(any(test, feature = "test_support"))]
    #[must_use]
    /// Construct test-only authority without transitive metadata search directories.
    pub fn new(artifact_root: PathBuf, leaves: Vec<OvenRustcRegistryLeaf>) -> Self {
        Self::new_with_trusted_dependency_search_paths(artifact_root, leaves, Vec::new())
    }

    /// Construct a catalog whose transitive Rust metadata may be located only in already verified plan paths.
    #[must_use]
    pub fn new_with_trusted_dependency_search_paths(
        artifact_root: PathBuf,
        leaves: Vec<OvenRustcRegistryLeaf>,
        dependency_search_paths: Vec<PathBuf>,
    ) -> Self {
        Self {
            entries: leaves
                .into_iter()
                .map(|leaf| OvenRegistryLeafAuthorityEntry {
                    artifact_root: artifact_root.clone(),
                    leaf,
                    dependency_search_paths: dependency_search_paths.clone(),
                })
                .collect(),
        }
    }

    /// Construct authority for a complete plan materialized across independently leased immutable roots.
    ///
    /// The caller must first validate the unfragmented manifest and materialize every declared artifact through
    /// [`OvenRustcArtifactManifest::materialize_trusted_store_composed`]. A registry leaf's source tree may be
    /// byte-identical to the base while its target rlib belongs to the extension, so fragment-local catalogs are
    /// intentionally insufficient here. Each entry still names only the root containing its verified rlib; all
    /// transitive metadata directories come from the already verified composed plan.
    #[must_use]
    pub fn from_composed_plan(
        entries: Vec<(PathBuf, OvenRustcRegistryLeaf)>,
        artifact_plan: &OvenRustcArtifactPlan,
    ) -> Option<Self> {
        (!entries.is_empty()).then(|| Self {
            entries: entries
                .into_iter()
                .map(|(artifact_root, leaf)| OvenRegistryLeafAuthorityEntry {
                    artifact_root,
                    leaf,
                    dependency_search_paths: artifact_plan.dependency_search_paths.clone(),
                })
                .collect(),
        })
    }

    #[must_use]
    /// Join registry-leaf catalogs from independently sealed sources into one lookup surface.
    ///
    /// This does not itself decide compatibility. Callers reconcile shared package facts before joining catalogs,
    /// and [`select_sealed_registry_leaf`] verifies that every requirement-matching copy is either one portable
    /// selected unit or byte-exact legacy evidence. Input order is authority order: provider catalogs may therefore
    /// precede a consumer catalog so caller-owned dependents compile against the already-sealed provider unit.
    pub fn aggregate(authorities: impl IntoIterator<Item = Self>) -> Self {
        Self {
            entries: authorities
                .into_iter()
                .flat_map(|authority| authority.entries)
                .collect(),
        }
    }

    /// Return the first package name this authority's own registry leaves would silently link as a second,
    /// incompatible compiled instance of a crate `plan` already links explicitly.
    ///
    /// A caller-owned provider's own registry closure (for example a query-engine library's own third-party
    /// dependency graph) is baked through an independent Cargo resolve from the consumer/SDK's own closure. Both can
    /// legitimately depend on "the same" package at the same version -- most dangerously an async runtime such as
    /// `tokio` -- yet resolve to two byte-distinct compiled artifacts, because a compiled crate's identity depends on
    /// its full compilation context, not only its declared version. Neither [`select_sealed_registry_leaf`] (which
    /// only sees one closure's own catalog) nor [`Self::aggregate`] (which only decides what is *discoverable*, not
    /// what is safe to *use*) can catch this: the danger appears only once a provider's own extern for the shared
    /// package and the consumer's own extern for it are compared directly. This does that comparison, checked
    /// against real evidence: linking a provider's own DataFusion/Tokio closure into the same binary as the SDK's
    /// own Tokio-based `block_on` support (RFC 048/114) is exactly what produced a real "no reactor running" panic
    /// at runtime, discovered as two distinct `tokio` symbol-mangled crate instances in the same linked executable.
    /// A build-time refusal here is far cheaper than that panic. Without a second authority this deliberately
    /// conservative compatibility check remains byte-exact; callers that have portable RFC 124 selected-unit
    /// identities use [`Self::first_conflicting_package_with_reconciled_authority`] instead.
    pub fn first_conflicting_package_with(
        &self,
        plan: &OvenRustcArtifactPlan,
    ) -> Result<Option<String>, OvenRustcError> {
        self.first_conflicting_package_with_reconciled_authority(plan, None)
    }

    /// Return the first named plan extern that cannot be reconciled through another sealed unit authority.
    ///
    /// `reconciled` is the consumer authority that supplied the plan. When it carries the same portable selected-unit
    /// identity and semantic facts as this provider leaf, both records denote one RFC 124 unit even if independent
    /// publisher staging made their payload digests differ. The plan may keep its already compiled representative;
    /// every caller-owned dependent compiled in this command is then built against that representative. A legacy
    /// leaf without an identity, or any semantic mismatch, remains subject to the byte-exact refusal.
    pub fn first_conflicting_package_with_reconciled_authority(
        &self,
        plan: &OvenRustcArtifactPlan,
        reconciled: Option<&Self>,
    ) -> Result<Option<String>, OvenRustcError> {
        for entry in &self.entries {
            let Some((_, existing_path)) = plan
                .externs
                .iter()
                .find(|(crate_name, _)| *crate_name == entry.leaf.crate_name)
            else {
                continue;
            };
            let candidate_path = safe_artifact_path(
                &entry.artifact_root,
                &entry.leaf.artifact.relative_path,
                "registry leaf",
            )?;
            if fs::canonicalize(&candidate_path).ok().as_deref() == fs::canonicalize(existing_path).ok().as_deref() {
                continue;
            }
            if reconciled
                .map(|authority| authority.has_compatible_unit_at_path(&entry.leaf, existing_path))
                .transpose()?
                .unwrap_or(false)
            {
                continue;
            }
            let existing_bytes = fs::read(existing_path).map_err(|source| OvenRustcError::Io {
                path: existing_path.clone(),
                source,
            })?;
            if digest_bytes(&existing_bytes) != entry.leaf.artifact.digest {
                return Ok(Some(entry.leaf.package.clone()));
            }
        }
        Ok(None)
    }

    /// Return the first package both authorities carry at the same version but as byte-distinct compiled artifacts.
    ///
    /// [`Self::first_conflicting_package_with`] only sees packages a plan links as a *named* `--extern`, but a shared
    /// package can just as easily enter both sides transitively -- the real `tokio` duplication that motivated these
    /// checks was never a named extern of either compile; both copies loaded purely through `-L dependency=...`
    /// metadata search from their respective dependents. Whenever the consumer's dependents and a provider's
    /// dependents both end up in one link (which is always true for a caller-owned provider: the SDK runtime and the
    /// provider library are both linked), incompatible records for one package version mean the closures cannot
    /// share one compiled unit. Different versions remain distinct Cargo units. Return the first diverging shared
    /// package together with the artifact root of the copy `other` pins.
    ///
    /// The package name alone tells a reader what conflicts but not what to change. The pinning artifact root names
    /// the already-compiled contributor whose copy cannot move, which is the difference between "two versions of
    /// this crate exist" and "this provider was built against that one and would have to be rebuilt to agree".
    pub fn first_diverging_shared_package_pin(&self, other: &Self) -> Option<(String, PathBuf)> {
        for entry in &self.entries {
            for candidate in &other.entries {
                if entry.leaf.package == candidate.leaf.package
                    && entry.leaf.version == candidate.leaf.version
                    && !registry_units_are_compatible(&entry.leaf, &candidate.leaf)
                    && !registry_units_are_byte_equivalent(&entry.leaf, &candidate.leaf)
                {
                    return Some((entry.leaf.package.clone(), candidate.artifact_root.clone()));
                }
            }
        }
        None
    }

    /// Return whether `path` is this authority's selected representative of the same portable unit as `leaf`.
    fn has_compatible_unit_at_path(&self, leaf: &OvenRustcRegistryLeaf, path: &Path) -> Result<bool, OvenRustcError> {
        let selected_path = fs::canonicalize(path).map_err(|source| OvenRustcError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        for candidate in &self.entries {
            if !registry_units_are_compatible(&candidate.leaf, leaf) {
                continue;
            }
            let candidate_path = safe_artifact_path(
                &candidate.artifact_root,
                &candidate.leaf.artifact.relative_path,
                "registry leaf",
            )?;
            let candidate_path = fs::canonicalize(&candidate_path).map_err(|source| OvenRustcError::Io {
                path: candidate_path,
                source,
            })?;
            if candidate_path == selected_path {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

/// Return whether two sealed leaves denote one interchangeable RFC 124 unit.
///
/// The selected-unit identity is the portable compiler-input key available on publisher-sealed registry leaves. It
/// excludes physical staging paths while binding the source graph, features, domain, target intent and dependencies.
/// Comparing the explicit leaf semantics as well makes a malformed catalog fail closed instead of trusting a copied
/// identity string whose surrounding facts disagree.
fn registry_units_are_compatible(left: &OvenRustcRegistryLeaf, right: &OvenRustcRegistryLeaf) -> bool {
    left.selected_unit_identity.is_some()
        && left.selected_unit_identity == right.selected_unit_identity
        && left.package == right.package
        && left.version == right.version
        && left.crate_name == right.crate_name
        && left.domain == right.domain
        && left.crate_kind == right.crate_kind
        && left.features == right.features
        && left.source == right.source
}

/// Return whether legacy records prove compatibility through identical semantic facts and compiled bytes.
fn registry_units_are_byte_equivalent(left: &OvenRustcRegistryLeaf, right: &OvenRustcRegistryLeaf) -> bool {
    left.package == right.package
        && left.version == right.version
        && left.crate_name == right.crate_name
        && left.domain == right.domain
        && left.crate_kind == right.crate_kind
        && left.features == right.features
        && left.source == right.source
        && left.artifact.digest == right.artifact.digest
}

/// One digest-verified registry leaf plus the plan directories Rustc may use solely for its transitive metadata.
#[derive(Debug)]
pub struct ResolvedSealedRegistryLeaf {
    pub artifact: PathBuf,
    pub dependency_search_paths: Vec<PathBuf>,
}

/// Resolve one registry dependency from the selected Loaf's sealed catalog.
pub fn resolve_sealed_registry_leaf(
    dependency: &DependencySpec,
    authority: Option<&OvenRegistryLeafAuthority>,
    profile: &str,
) -> Result<PathBuf, OvenRustcError> {
    Ok(resolve_sealed_registry_leaf_with_search_paths(dependency, authority, profile)?.artifact)
}

/// Verify that a registry dependency already represented by a selected direct-Rustc extern is semantically valid.
///
/// A matching Rust crate name is not sufficient authority: the caller's package, version, and features still have to
/// match one digest-verified registry leaf from a receipt-compatible native catalog. This validates that contract
/// without compiling a duplicate caller-owned `--extern` or consulting Cargo state.
pub fn validate_sealed_registry_leaf(
    dependency: &DependencySpec,
    authority: Option<&OvenRegistryLeafAuthority>,
    profile: &str,
) -> Result<(), OvenRustcError> {
    let _ = select_sealed_registry_leaf(dependency, authority, profile)?;
    Ok(())
}

/// Verify a registry dependency against the exact materialized extern already selected for its alias.
///
/// This is the execution-time boundary for every selected plan shape. The plan has already fixed an alias to one
/// immutable artifact path; validating that path's sealed leaf avoids repeating a highest-compatible-version choice
/// that could disagree when the catalog contains two semver-compatible package versions.
pub fn validate_selected_sealed_registry_leaf(
    dependency: &DependencySpec,
    selected_artifact: &Path,
    authority: Option<&OvenRegistryLeafAuthority>,
    profile: &str,
) -> Result<(), OvenRustcError> {
    let authority = authority.ok_or_else(|| OvenRustcError::InvalidInput {
        field: "Oven registry Rust dependency",
        message: format!(
            "`{}` has no receipt-bound Loaf registry catalog",
            dependency.package.as_deref().unwrap_or(&dependency.crate_name)
        ),
    })?;
    let selected_artifact = verified_regular_file(selected_artifact, "selected registry artifact")?;
    let selected_artifact = fs::canonicalize(&selected_artifact).map_err(|source| OvenRustcError::Io {
        path: selected_artifact,
        source,
    })?;
    let mut path_matches = Vec::new();
    for entry in &authority.entries {
        let artifact_path = safe_artifact_path(
            &entry.artifact_root,
            &entry.leaf.artifact.relative_path,
            "sealed registry leaf",
        )?;
        if artifact_path == selected_artifact {
            path_matches.push(entry);
        }
    }
    let [selected] = path_matches.as_slice() else {
        return Err(OvenRustcError::InvalidInput {
            field: "Oven registry Rust dependency",
            message: format!(
                "selected extern `{}` has {} exact receipt-bound registry leaf records",
                selected_artifact.display(),
                path_matches.len()
            ),
        });
    };
    let package = dependency.package.as_deref().unwrap_or(&dependency.crate_name);
    let requirement_text = dependency
        .version
        .as_deref()
        .ok_or_else(|| OvenRustcError::InvalidInput {
            field: "Oven registry Rust dependency",
            message: format!("`{package}` has no declared version requirement"),
        })?;
    let requirement = VersionReq::parse(requirement_text).map_err(|error| OvenRustcError::InvalidInput {
        field: "Oven registry Rust dependency",
        message: format!("`{package}` has invalid version requirement `{requirement_text}`: {error}"),
    })?;
    let available_features = selected.leaf.features.iter().collect::<BTreeSet<_>>();
    let requested_features = dependency.features.iter().collect::<BTreeSet<_>>();
    if selected.leaf.package != package
        || !Version::parse(&selected.leaf.version).is_ok_and(|version| requirement.matches(&version))
        || !requested_features.is_subset(&available_features)
    {
        return Err(OvenRustcError::InvalidInput {
            field: "Oven registry Rust dependency",
            message: format!(
                "selected extern `{}` does not satisfy `{package}` requirement `{requirement_text}` and its requested features",
                selected_artifact.display()
            ),
        });
    }
    let relative_path = Path::new(&selected.leaf.artifact.relative_path);
    let selected_profile = relative_path
        .parent()
        .filter(|parent| parent.file_name().is_some_and(|name| name == "deps"))
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .and_then(|name| name.to_str());
    if let Some(selected_profile) = selected_profile
        && selected_profile != profile
    {
        return Err(OvenRustcError::InvalidInput {
            field: "Oven registry Rust dependency",
            message: format!(
                "selected extern `{}` was baked for profile `{}`, not `{profile}`",
                selected_artifact.display(),
                selected_profile
            ),
        });
    }
    Ok(())
}

/// Select one semantically compatible sealed registry leaf without re-reading its artifact bytes.
///
/// The caller either uses this to validate a selected direct-Rustc extern that the immutable plan has already checked,
/// or [`resolve_sealed_registry_leaf_with_search_paths`] below to verify and attach a new caller-owned leaf.
pub fn select_sealed_registry_leaf<'a>(
    dependency: &DependencySpec,
    authority: Option<&'a OvenRegistryLeafAuthority>,
    profile: &str,
) -> Result<&'a OvenRegistryLeafAuthorityEntry, OvenRustcError> {
    let authority = authority.ok_or_else(|| OvenRustcError::InvalidInput {
        field: "Oven registry Rust dependency",
        message: format!(
            "`{}` has no receipt-bound Loaf registry catalog; prepare an explicit Oven-native closure",
            dependency.package.as_deref().unwrap_or(&dependency.crate_name)
        ),
    })?;
    let package_name = dependency.package.as_deref().unwrap_or(&dependency.crate_name);
    let requirement_text = dependency
        .version
        .as_deref()
        .ok_or_else(|| OvenRustcError::InvalidInput {
            field: "Oven registry Rust dependency",
            message: format!("`{package_name}` has no declared version requirement"),
        })?;
    let requirement = VersionReq::parse(requirement_text).map_err(|error| OvenRustcError::InvalidInput {
        field: "Oven registry Rust dependency",
        message: format!("`{package_name}` has invalid version requirement `{requirement_text}`: {error}"),
    })?;
    let requested_features = dependency.features.iter().collect::<BTreeSet<_>>();
    let mut candidates = authority
        .entries
        .iter()
        .filter_map(|entry| {
            let leaf = &entry.leaf;
            // A consumer's `--extern` names a target library. Host libraries and procedural macros are sealed
            // beside it for the runtime foundation; they are not what a registry dependency edge selects here.
            if leaf.package != package_name
                || leaf.domain != super::OvenRustcRegistryLeafDomain::Target
                || leaf.crate_kind != super::OvenRustcRegistryLeafKind::Rlib
            {
                return None;
            }
            let version = Version::parse(&leaf.version).ok()?;
            let available_features = leaf.features.iter().collect::<BTreeSet<_>>();
            (requirement.matches(&version) && requested_features.is_subset(&available_features))
                .then_some((version, entry))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|(left_version, _), (right_version, _)| right_version.cmp(left_version));
    // A suite ships separate debug and release Loaf catalogs. Prefer the matching profile whenever its sealed
    // catalog contains this dependency; fixtures use short synthetic paths, so retain the complete catalog when no
    // profile-qualified artifact exists.
    let profile_marker = format!("/{profile}/deps/");
    if candidates
        .iter()
        .any(|(_, entry)| entry.leaf.artifact.relative_path.contains(&profile_marker))
    {
        candidates.retain(|(_, entry)| entry.leaf.artifact.relative_path.contains(&profile_marker));
    }
    let Some((selected_version, selected)) = candidates.first().map(|(version, entry)| (version.clone(), *entry))
    else {
        return Err(OvenRustcError::InvalidInput {
            field: "Oven registry Rust dependency",
            message: format!(
                "`{package_name}` requirement `{requirement_text}` has no compatible receipt-bound Loaf registry leaf; prepare an explicit Oven-native closure"
            ),
        });
    };
    let same_compilation = candidates
        .iter()
        .filter(|(version, _)| version == &selected_version)
        .all(|(_, entry)| {
            registry_units_are_compatible(&entry.leaf, &selected.leaf)
                || registry_units_are_byte_equivalent(&entry.leaf, &selected.leaf)
        });
    if !same_compilation {
        return Err(OvenRustcError::InvalidInput {
            field: "Oven registry Rust dependency",
            message: format!(
                "`{package_name}` version `{selected_version}` resolves to multiple receipt-bound Loaf registry leaves; prepare an explicit Oven-native closure"
            ),
        });
    }
    Ok(selected)
}

/// Resolve a registry leaf and the pre-verified search directory required to load its transitive metadata.
pub fn resolve_sealed_registry_leaf_with_search_paths(
    dependency: &DependencySpec,
    authority: Option<&OvenRegistryLeafAuthority>,
    profile: &str,
) -> Result<ResolvedSealedRegistryLeaf, OvenRustcError> {
    let selected = select_sealed_registry_leaf(dependency, authority, profile)?;
    let package_name = dependency.package.as_deref().unwrap_or(&dependency.crate_name);
    // A Cargo publisher can retain byte-distinct payloads for one portable selected unit across independently staged
    // native closures. The RFC 124 identity, not an artifact filename containing staging-sensitive metadata, is the
    // authority for choosing one representative. Legacy records require byte-exact semantic evidence;
    // different portable identities remain fail-closed above.
    let artifact = &selected.leaf.artifact;
    let artifact_path = safe_artifact_path(&selected.artifact_root, &artifact.relative_path, "registry leaf")?;
    let bytes = fs::read(&artifact_path).map_err(|source| OvenRustcError::Io {
        path: artifact_path.clone(),
        source,
    })?;
    if digest_bytes(&bytes) != artifact.digest {
        return Err(OvenRustcError::InvalidInput {
            field: "Oven registry Rust dependency",
            message: format!(
                "sealed registry leaf `{package_name}` at {} failed digest verification",
                artifact_path.display()
            ),
        });
    }
    let extension = artifact_path.extension().and_then(|extension| extension.to_str());
    if extension != Some("rlib") {
        return Err(OvenRustcError::InvalidInput {
            field: "Oven registry Rust dependency",
            message: format!(
                "sealed registry leaf `{package_name}` at {} is not an rlib",
                artifact_path.display()
            ),
        });
    }
    let artifact_parent = artifact_path.parent().ok_or_else(|| OvenRustcError::InvalidInput {
        field: "Oven registry Rust dependency",
        message: format!(
            "sealed registry leaf `{package_name}` at {} has no dependency directory",
            artifact_path.display()
        ),
    })?;
    let mut dependency_search_paths = selected
        .dependency_search_paths
        .iter()
        .map(|path| canonical_directory(path, "registry leaf dependency search path"))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|path| path == artifact_parent)
        .collect::<Vec<_>>();
    dependency_search_paths.sort();
    dependency_search_paths.dedup();
    if !selected.dependency_search_paths.is_empty() && dependency_search_paths.is_empty() {
        return Err(OvenRustcError::InvalidInput {
            field: "Oven registry Rust dependency",
            message: format!(
                "sealed registry leaf `{package_name}` at {} is outside its verified dependency search paths",
                artifact_path.display()
            ),
        });
    }
    Ok(ResolvedSealedRegistryLeaf {
        artifact: artifact_path,
        dependency_search_paths,
    })
}

/// Resolve one manifest-recorded Loaf artifact without allowing the leaf catalog to escape its sealed root.
pub fn safe_artifact_path(
    artifact_root: &Path,
    relative_path: &str,
    kind: &'static str,
) -> Result<PathBuf, OvenRustcError> {
    let normalized = normalized_relative_path(relative_path, kind)?;
    let root = canonical_directory(artifact_root, "registry leaf artifact root")?;
    let artifact = verified_regular_file(&root.join(normalized), kind)?;
    if !artifact.starts_with(&root) {
        return Err(OvenRustcError::InvalidInput {
            field: "Oven registry Rust dependency",
            message: format!("{kind} artifact {} escapes its sealed root", artifact.display()),
        });
    }
    Ok(artifact)
}
