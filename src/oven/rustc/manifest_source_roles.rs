//! Source-search roles: which registry source trees a plan may read, bound to which crate, at which root.
//!
//! A role names one physical source root the plan materialized and the crate it serves; projecting a plan for a
//! role it never materialized is refused, so no search path is ever synthesized from a name alone.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::{
    OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION, OVEN_RUSTC_LEGACY_ARTIFACT_MANIFEST_SCHEMA_VERSION,
    OvenRustcArtifactExtern, OvenRustcArtifactManifest, OvenRustcError, OvenRustcSourcePathProjection,
    OvenRustcSupportingArtifact, TrustedShape, artifact_is_below_search_path, expected_artifacts,
    normalized_relative_path, rust_library_crate_name, trusted_file,
};

impl OvenRustcArtifactManifest {
    /// Borrow named members of an original declared role without granting physical execution authority.
    ///
    /// The existing projection validates the declaration once. Returned records still require an original leased
    /// owner and physical attachment before execution; absent roles cannot use legacy broad projection.
    ///
    /// Gate 6 of RFC 119 is the reader. Unlike the rest of this substrate it is annotated rather than moved into a
    /// submodule, because it hangs off `OvenRustcArtifactManifest`, which is live.
    #[allow(dead_code, reason = "Gate 6 of RFC 119 is the reader; see the note above")]
    pub(crate) fn named_artifacts_for_source_role(
        &self,
        source_role: &str,
    ) -> Result<Vec<&OvenRustcArtifactExtern>, OvenRustcError> {
        if !self.entrypoint_externs.contains_key(source_role) {
            return Err(OvenRustcError::InvalidInput {
                field: "native input source role",
                message: format!("`{source_role}` is not an original declared role of this foundation"),
            });
        }
        let projected = self.for_source_evidence(source_role)?;
        Ok(self
            .externs
            .iter()
            .filter(|artifact| projected.externs.contains(artifact))
            .collect())
    }

    /// Validate source-role search evidence without discovering files or widening a legacy plan.
    pub(super) fn validate_source_search_roles(&self) -> Result<(), OvenRustcError> {
        let invalid = |message: String| OvenRustcError::InvalidInput {
            field: "artifact manifest source search closure",
            message,
        };
        if self.schema_version == OVEN_RUSTC_LEGACY_ARTIFACT_MANIFEST_SCHEMA_VERSION {
            return if self.entrypoint_dependency_search_paths.is_empty() {
                Ok(())
            } else {
                Err(invalid(
                    "legacy schema must not declare source-role search evidence".to_string(),
                ))
            };
        }
        if !self
            .entrypoint_externs
            .keys()
            .eq(self.entrypoint_dependency_search_paths.keys())
        {
            return Err(invalid(
                "every declared source role must have exactly one publisher-selected search closure".to_string(),
            ));
        }
        let expected = expected_artifacts(self)?;
        for (key, closure) in &self.entrypoint_dependency_search_paths {
            let mut origins = BTreeSet::new();
            for projection in &closure.legacy_projections {
                let valid_digest = projection
                    .source_manifest_digest
                    .strip_prefix("sha256:")
                    .is_some_and(|value| {
                        value.len() == 64
                            && value
                                .bytes()
                                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                    });
                if projection.source_schema_version != OVEN_RUSTC_LEGACY_ARTIFACT_MANIFEST_SCHEMA_VERSION
                    || !valid_digest
                    || projection.source_evidence_key.trim().is_empty()
                    || !origins.insert((&projection.source_manifest_digest, &projection.source_evidence_key))
                {
                    return Err(invalid(format!(
                        "source evidence `{key}` has invalid or duplicate legacy projection provenance"
                    )));
                }
            }
            for directories in std::iter::once(&closure.publisher_paths).chain(
                closure
                    .legacy_projections
                    .iter()
                    .map(|projection| &projection.dependency_search_paths),
            ) {
                let mut seen = BTreeSet::new();
                for directory in directories {
                    let path = &directory.relative_path;
                    let normalized = normalized_relative_path(path, "source search closure")?;
                    if normalized != *path || !seen.insert(path) || !self.dependency_search_paths.contains(path) {
                        return Err(invalid(format!(
                            "source evidence `{key}` declares an invalid, duplicate, or unlisted search path `{path}`"
                        )));
                    }
                    if directory.artifacts.is_empty() {
                        return Err(invalid(format!(
                            "source evidence `{key}` search path `{path}` has no declared member"
                        )));
                    }
                    let mut members = BTreeSet::new();
                    for member in &directory.artifacts {
                        let normalized = normalized_relative_path(&member.relative_path, "source search member")?;
                        if normalized != member.relative_path
                            || !members.insert(&member.relative_path)
                            || !artifact_is_below_search_path(&member.relative_path, path)
                            || expected.get(&member.relative_path) != Some(&member.digest)
                        {
                            let declared = expected
                                .get(&member.relative_path)
                                .map_or_else(|| "undeclared".to_string(), |digest| format!("declared {digest}"));
                            return Err(invalid(format!(
                                "source evidence `{key}` search path `{path}` has invalid or unowned member `{}` \
                                 (closure claims {}, manifest {declared})",
                                member.relative_path, member.digest
                            )));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Report whether `key` already binds the crate that `relative_path` holds, through an explicit extern at a
    /// different path.
    ///
    /// The isolation rule refuses a directory holding an artifact the role never selected, because rustc would
    /// otherwise reach it through `-L dependency`. That reasoning does not apply to a crate the role passes as an
    /// explicit `--extern`: the name is already resolved to one exact path, and a same-named file beside it cannot
    /// be selected implicitly.
    ///
    /// This is exactly the arrangement a project extension produces in the conservative regime. The extension keeps
    /// its own `incan_stdlib` while still drawing registry leaves from the base Loaf, whose directory carries the
    /// base's runtime -- the one artifact `with_release_cohort_from_base` deliberately declines. One mechanism drops
    /// that claim and the other demanded it back, so a bake that was correctly composed could not be materialized.
    ///
    /// Deliberately narrow: it admits only a crate this role explicitly externs, and only from a different path.
    /// An unrelated co-resident, or a second copy of a crate the role does not extern, still refuses.
    pub(super) fn role_binds_crate_elsewhere(&self, key: &str, relative_path: &str) -> bool {
        let Some(crate_name) = rust_library_crate_name(relative_path) else {
            return false;
        };
        let bound_by_role = self
            .entrypoint_externs
            .get(key)
            .is_some_and(|names| names.iter().any(|name| name == crate_name));
        bound_by_role
            && self
                .externs
                .iter()
                .any(|artifact| artifact.crate_name == crate_name && artifact.relative_path != relative_path)
    }

    /// Bind selected members to clean assigned directories or exact admitted copies under retained leases.
    pub(super) fn bind_source_search_roles(
        &self,
        file_roots: &BTreeMap<String, PathBuf>,
        inventories: &BTreeMap<(PathBuf, String), BTreeMap<String, String>>,
    ) -> Result<Option<OvenRustcSourcePathProjection>, OvenRustcError> {
        if self.schema_version == OVEN_RUSTC_LEGACY_ARTIFACT_MANIFEST_SCHEMA_VERSION {
            return Ok(None);
        }
        let mut roles = BTreeMap::new();
        let mut trusted_parents = BTreeMap::new();
        for (key, closure) in &self.entrypoint_dependency_search_paths {
            let selected_members = closure
                .directories()
                .flat_map(|directory| &directory.artifacts)
                .map(|artifact| (artifact.relative_path.clone(), artifact.digest.clone()))
                .collect::<BTreeMap<_, _>>();
            let mut selected = BTreeSet::new();
            for directory in closure.directories() {
                for member in &directory.artifacts {
                    let root = file_roots
                        .get(&member.relative_path)
                        .ok_or_else(|| OvenRustcError::InvalidInput {
                            field: "materialized source search closure",
                            message: format!(
                                "source evidence `{key}` has no assigned owner for `{}`",
                                member.relative_path
                            ),
                        })?;
                    let canonical = (root.clone(), directory.relative_path.clone());
                    let clean = |inventory: &BTreeMap<String, String>| {
                        inventory.get(&member.relative_path) == Some(&member.digest)
                            && inventory.iter().all(|(path, digest)| {
                                selected_members.get(path) == Some(digest) || self.role_binds_crate_elsewhere(key, path)
                            })
                    };
                    let chosen = if inventories.get(&canonical).is_some_and(clean) {
                        &canonical
                    } else {
                        inventories
                            .iter()
                            .find(|((_, relative), inventory)| relative == &directory.relative_path && clean(inventory))
                            .map(|(candidate, _)| candidate)
                            .ok_or_else(|| {
                                // "a co-resident unselected artifact" names a condition, not a cause. Say which
                                // artifact and why it disqualified the directory, so the reader can look at the one
                                // file that matters instead of every member of a Cargo deps directory.
                                let blame = inventories.get(&canonical).map_or_else(
                                    || "its canonical directory has no recorded inventory".to_string(),
                                    |inventory| match inventory.get(&member.relative_path) {
                                        None => format!("`{}` is absent from it", member.relative_path),
                                        Some(found) if found != &member.digest => {
                                            format!("`{}` is present with a different digest", member.relative_path)
                                        }
                                        Some(_) => {
                                            let mut intruders = inventory
                                                .iter()
                                                .filter(|(path, digest)| selected_members.get(*path) != Some(*digest))
                                                .map(|(path, _)| path.as_str())
                                                .collect::<Vec<_>>();
                                            let total = intruders.len();
                                            intruders.truncate(3);
                                            format!(
                                                "it also holds {total} artifact(s) this role never selected, such as {}",
                                                intruders.join(", ")
                                            )
                                        }
                                    },
                                );
                                OvenRustcError::InvalidInput {
                                    field: "materialized source search closure",
                                    message: format!(
                                        "source evidence `{key}` cannot isolate selected member `{}`: {blame}, and no other admitted directory holds a clean copy",
                                        member.relative_path
                                    ),
                                }
                            })?
                    };
                    if chosen != &canonical {
                        // Only an actually used alternate copy needs another containment/file check; no rehash or scan.
                        trusted_file(
                            &chosen.0,
                            &member.relative_path,
                            "admitted source search member",
                            &mut trusted_parents,
                            TrustedShape::Checked,
                        )?;
                    }
                    selected.insert(chosen.0.join(&chosen.1));
                }
            }
            roles.insert(key.clone(), (closure.clone(), selected));
        }
        Ok(Some(OvenRustcSourcePathProjection {
            declared: inventories.keys().map(|(root, relative)| root.join(relative)).collect(),
            roles,
        }))
    }

    /// Bind a complete single-root publication after existing artifact and directory validation.
    pub(super) fn source_search_roles_at_root(
        &self,
        root: &Path,
    ) -> Result<Option<OvenRustcSourcePathProjection>, OvenRustcError> {
        let declared = expected_artifacts(self)?;
        let file_roots = declared.keys().map(|path| (path.clone(), root.to_path_buf())).collect();
        let inventories = self
            .dependency_search_paths
            .iter()
            .map(|directory| {
                (
                    (root.to_path_buf(), directory.clone()),
                    declared
                        .iter()
                        .filter(|(path, _)| artifact_is_below_search_path(path, directory))
                        .map(|(path, digest)| (path.clone(), digest.clone()))
                        .collect(),
                )
            })
            .collect();
        self.bind_source_search_roles(&file_roots, &inventories)
    }

    /// Select the exact direct root externs authorized for one receipt source target.
    ///
    /// The unselected extern artifacts remain declared supporting inputs so strict search-directory completeness and
    /// publisher-time digest verification still cover the entire immutable compatibility closure.
    pub(super) fn for_source_evidence(&self, source_evidence_key: &str) -> Result<Self, OvenRustcError> {
        self.validate_shape(&self.intent)?;
        let Some(allowed_names) = self.entrypoint_externs.get(source_evidence_key) else {
            return Ok(self.clone());
        };
        let allowed = allowed_names.iter().cloned().collect::<BTreeSet<_>>();
        let available = self
            .externs
            .iter()
            .map(|artifact| artifact.crate_name.clone())
            .collect::<BTreeSet<_>>();
        let missing = allowed
            .iter()
            .filter(|crate_name| !available.contains(*crate_name))
            .cloned()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(OvenRustcError::InvalidInput {
                field: "artifact manifest entrypoint externs",
                message: format!(
                    "source evidence `{source_evidence_key}` names undeclared extern(s): {}",
                    missing.join(", ")
                ),
            });
        }
        let mut selected = self.clone();
        // A helper-only dependency directory may contain a second build of a crate that has the same stable Rust
        // identity as a generated-root dependency. Keeping that directory on `-L dependency` after its only direct
        // roots are projected away still lets Rustc discover the conflicting copy. Retain every mixed or
        // transitive-only directory, but remove a directory whose declared direct roots are all excluded.
        let excluded_dependency_search_paths = self
            .dependency_search_paths
            .iter()
            .filter(|search_path| {
                let direct_roots = self
                    .externs
                    .iter()
                    .filter(|artifact| artifact_is_below_search_path(&artifact.relative_path, search_path))
                    .collect::<Vec<_>>();
                !direct_roots.is_empty()
                    && direct_roots
                        .iter()
                        .all(|artifact| !allowed.contains(&artifact.crate_name))
            })
            .cloned()
            .collect::<BTreeSet<_>>();
        let retained = selected
            .externs
            .iter()
            .filter(|artifact| !allowed.contains(&artifact.crate_name))
            .map(|artifact| OvenRustcSupportingArtifact {
                relative_path: artifact.relative_path.clone(),
                digest: artifact.digest.clone(),
            })
            .collect::<Vec<_>>();
        selected
            .externs
            .retain(|artifact| allowed.contains(&artifact.crate_name));
        if self.schema_version == OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION {
            let paths = self
                .entrypoint_dependency_search_paths
                .get(source_evidence_key)
                .ok_or_else(|| OvenRustcError::InvalidInput {
                    field: "artifact manifest source search closure",
                    message: format!(
                        "source evidence `{source_evidence_key}` has no publisher-selected search closure"
                    ),
                })?;
            selected.dependency_search_paths = paths.paths().cloned().collect::<BTreeSet<_>>().into_iter().collect();
            selected.entrypoint_externs.retain(|key, _| key == source_evidence_key);
            selected
                .entrypoint_dependency_search_paths
                .retain(|key, _| key == source_evidence_key);
        } else {
            selected
                .dependency_search_paths
                .retain(|search_path| !excluded_dependency_search_paths.contains(search_path));
        }
        selected.supporting_artifacts.extend(retained);
        Ok(selected)
    }
}
