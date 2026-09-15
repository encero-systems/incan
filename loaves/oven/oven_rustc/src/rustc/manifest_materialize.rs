//! Materializing a sealed artifact plan into the paths rustc receives.
//!
//! The full materialization rehashes every declared file; the trusted forms take the publisher's digests as given
//! and check only shape, and a closure proof lets a later process skip even that. Composed materialization joins a
//! base cohort with an extension's own closure.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::{
    OVEN_CLOSURE_PROOF_SCHEMA_VERSION, OvenBuildIntent, OvenClosureProof, OvenRustcArtifactManifest,
    OvenRustcArtifactPlan, OvenRustcAuxiliaryTargetPlan, OvenRustcError, OvenRustcMaterializedArtifact,
    OvenTrustedRustcArtifactRoot, OvenTrustedRustcSearchRoot, TrustedShape, admitted_search_inventory,
    artifact_is_below_search_path, canonical_directory, expected_artifacts, materialize_search_paths,
    normalized_relative_path, trusted_file, trusted_materialize_search_paths, validate_publisher_search_paths,
    validate_rust_identifier, validated_compile_environment, verified_file,
};

impl OvenRustcArtifactManifest {
    /// Verify and materialize exact compiler inputs without scanning Cargo output or resolving dependencies.
    pub fn materialize(
        &self,
        artifact_root: &Path,
        expected_intent: &OvenBuildIntent,
    ) -> Result<OvenRustcArtifactPlan, OvenRustcError> {
        self.validate_shape(expected_intent)?;
        let root = canonical_directory(artifact_root, "artifact root")?;
        let expected = expected_artifacts(self)?;
        let dependency_search_paths =
            materialize_search_paths(&root, &self.dependency_search_paths, "dependency search", &expected)?;
        let native_search_paths =
            materialize_search_paths(&root, &self.native_search_paths, "native search", &expected)?;
        for auxiliary in &self.vocab_auxiliary_targets {
            let _ = materialize_search_paths(
                &root,
                &auxiliary.dependency_search_paths,
                "vocab auxiliary dependency search",
                &expected,
            )?;
            for artifact in &auxiliary.externs {
                let _ = verified_file(
                    &root,
                    &artifact.relative_path,
                    &artifact.digest,
                    "vocab auxiliary extern",
                )?;
            }
        }
        let externs = self
            .externs
            .iter()
            .map(|artifact| {
                validate_rust_identifier(&artifact.crate_name)?;
                let path = verified_file(&root, &artifact.relative_path, &artifact.digest, "extern")?;
                Ok((artifact.crate_name.clone(), path))
            })
            .collect::<Result<Vec<_>, OvenRustcError>>()?;
        for artifact in &self.supporting_artifacts {
            verified_file(&root, &artifact.relative_path, &artifact.digest, "supporting")?;
        }
        Ok(OvenRustcArtifactPlan {
            source_path_projection: self.source_search_roles_at_root(&root)?,
            dependency_search_paths,
            native_search_paths,
            externs,
            compile_environment: validated_compile_environment(&self.compile_environment)?,
            caller_owned_library_digests: BTreeMap::new(),
        })
    }

    /// Return the complete verified publisher closure for atomic copying into a store-owned artifact root.
    pub fn materialized_artifacts(
        &self,
        artifact_root: &Path,
        expected_intent: &OvenBuildIntent,
    ) -> Result<Vec<OvenRustcMaterializedArtifact>, OvenRustcError> {
        self.validate_shape(expected_intent)?;
        let root = canonical_directory(artifact_root, "artifact root")?;
        let expected = expected_artifacts(self)?;
        validate_publisher_search_paths(&self.dependency_search_paths, "dependency search", &expected)?;
        validate_publisher_search_paths(&self.native_search_paths, "native search", &expected)?;
        for auxiliary in &self.vocab_auxiliary_targets {
            validate_publisher_search_paths(
                &auxiliary.dependency_search_paths,
                "vocab auxiliary dependency search",
                &expected,
            )?;
        }
        expected
            .into_iter()
            .map(|(relative_path, digest)| {
                Ok(OvenRustcMaterializedArtifact {
                    source_path: verified_file(&root, &relative_path, &digest, "materialized")?,
                    relative_path,
                })
            })
            .collect()
    }

    /// Materialize a plan that has already been atomically copied and digest-verified by the Oven store publisher.
    ///
    /// Normal consumers still verify the receipt, plan payload, artifact-root containment, regular-file shape, and
    /// active lease. They intentionally do not rehash every retained dependency: doing so turns a prepared test run
    /// into a multi-gigabyte integrity scan. `inspect oven` remains the explicit full-closure audit operation.
    pub fn materialize_trusted_store(
        &self,
        artifact_root: &Path,
        expected_intent: &OvenBuildIntent,
    ) -> Result<OvenRustcArtifactPlan, OvenRustcError> {
        self.materialize_trusted_store_with_shape(artifact_root, expected_intent, TrustedShape::Checked)
    }

    /// Materialize a sealed closure whose full shape check one process has already written down (#1546).
    ///
    /// `closure_identity` names the closure — for a Loaf, the digest of its manifest — and `proof_path` is where its
    /// [`OvenClosureProof`] lives. With a matching proof the per-file shape checks are skipped: each relative path
    /// is still normalized (no absolute or parent-escaping segment) and joined below the canonical root, but the ten
    /// thousand `symlink_metadata` and per-directory `canonicalize` calls that re-established a constant every
    /// process are not made. Without one, this is the full trusted materialization, and on success the proof is
    /// written for the next process. A proof that cannot be written costs nothing but the next process's full walk.
    pub fn materialize_proven_store(
        &self,
        artifact_root: &Path,
        expected_intent: &OvenBuildIntent,
        closure_identity: &str,
        proof_path: &Path,
    ) -> Result<OvenRustcArtifactPlan, OvenRustcError> {
        let artifact_count = self.declared_file_count();
        if OvenClosureProof::read_matching(proof_path, closure_identity, artifact_count).is_some() {
            return self.materialize_trusted_store_with_shape(artifact_root, expected_intent, TrustedShape::Proven);
        }
        let plan = self.materialize_trusted_store_with_shape(artifact_root, expected_intent, TrustedShape::Checked)?;
        let _ = OvenClosureProof {
            schema_version: OVEN_CLOSURE_PROOF_SCHEMA_VERSION,
            closure_identity: closure_identity.to_string(),
            artifact_count,
        }
        .write(proof_path);
        Ok(plan)
    }

    /// Count every file a full trusted materialization checks, so a proof binds to that exact declared set.
    pub(crate) fn declared_file_count(&self) -> u64 {
        let auxiliary_externs = self
            .vocab_auxiliary_targets
            .iter()
            .map(|auxiliary| auxiliary.externs.len())
            .sum::<usize>();
        u64::try_from(self.externs.len() + self.supporting_artifacts.len() + auxiliary_externs).unwrap_or(u64::MAX)
    }

    /// The trusted materialization, with the per-file shape checks either made or already proven.
    pub(crate) fn materialize_trusted_store_with_shape(
        &self,
        artifact_root: &Path,
        expected_intent: &OvenBuildIntent,
        shape: TrustedShape,
    ) -> Result<OvenRustcArtifactPlan, OvenRustcError> {
        self.validate_shape(expected_intent)?;
        let root = canonical_directory(artifact_root, "artifact root")?;
        let expected = expected_artifacts(self)?;
        let mut trusted_parents = BTreeMap::new();
        let dependency_search_paths = trusted_materialize_search_paths(
            &root,
            &self.dependency_search_paths,
            "dependency search",
            &expected,
            &mut trusted_parents,
            shape,
        )?;

        let native_search_paths = trusted_materialize_search_paths(
            &root,
            &self.native_search_paths,
            "native search",
            &expected,
            &mut trusted_parents,
            shape,
        )?;
        for auxiliary in &self.vocab_auxiliary_targets {
            let _ = trusted_materialize_search_paths(
                &root,
                &auxiliary.dependency_search_paths,
                "vocab auxiliary dependency search",
                &expected,
                &mut trusted_parents,
                shape,
            )?;
            for artifact in &auxiliary.externs {
                let _ = trusted_file(
                    &root,
                    &artifact.relative_path,
                    "vocab auxiliary extern",
                    &mut trusted_parents,
                    shape,
                )?;
            }
        }
        let externs = self
            .externs
            .iter()
            .map(|artifact| {
                Ok((
                    artifact.crate_name.clone(),
                    trusted_file(&root, &artifact.relative_path, "extern", &mut trusted_parents, shape)?,
                ))
            })
            .collect::<Result<Vec<_>, OvenRustcError>>()?;
        for artifact in &self.supporting_artifacts {
            trusted_file(
                &root,
                &artifact.relative_path,
                "supporting",
                &mut trusted_parents,
                shape,
            )?;
        }
        Ok(OvenRustcArtifactPlan {
            source_path_projection: self.source_search_roles_at_root(&root)?,
            dependency_search_paths,
            native_search_paths,
            externs,
            compile_environment: validated_compile_environment(&self.compile_environment)?,
            caller_owned_library_digests: BTreeMap::new(),
        })
    }

    /// Materialize one publisher-declared vocabulary cross-target closure from an already selected immutable root.
    ///
    /// This is intentionally not part of the ordinary [`OvenRustcArtifactPlan`]: normal host commands must never
    /// hand cross-target artifacts to Rustc. Vocabulary extraction names the target from its verified metadata and
    /// may receive only the exact matching auxiliary closure.
    pub fn materialize_trusted_vocab_auxiliary_target(
        &self,
        artifact_root: &Path,
        target: &str,
    ) -> Result<Option<OvenRustcAuxiliaryTargetPlan>, OvenRustcError> {
        let root = canonical_directory(artifact_root, "artifact root")?;
        let expected = expected_artifacts(self)?;
        let mut trusted_parents = BTreeMap::new();
        let Some(auxiliary) = self
            .vocab_auxiliary_targets
            .iter()
            .find(|auxiliary| auxiliary.target == target)
        else {
            return Ok(None);
        };
        let dependency_search_paths = trusted_materialize_search_paths(
            &root,
            &auxiliary.dependency_search_paths,
            "vocab auxiliary dependency search",
            &expected,
            &mut trusted_parents,
            TrustedShape::Checked,
        )?;
        let externs = auxiliary
            .externs
            .iter()
            .map(|artifact| {
                Ok((
                    artifact.crate_name.clone(),
                    trusted_file(
                        &root,
                        &artifact.relative_path,
                        "vocab auxiliary extern",
                        &mut trusted_parents,
                        TrustedShape::Checked,
                    )?,
                ))
            })
            .collect::<Result<Vec<_>, OvenRustcError>>()?;
        Ok(Some(OvenRustcAuxiliaryTargetPlan {
            dependency_search_paths,
            externs,
        }))
    }

    /// Materialize a complete direct-rustc plan from several separately bounded, actively leased Oven roots.
    ///
    /// The outer manifest remains the one target-specific execution contract. Each root contributes a disjoint
    /// publisher-declared fragment; this method rejects a missing, duplicate, or substituted path before passing
    /// any `-L` or `--extern` argument to Rustc.
    pub fn materialize_trusted_store_composed(
        &self,
        roots: &[OvenTrustedRustcArtifactRoot<'_>],
        expected_intent: &OvenBuildIntent,
    ) -> Result<OvenRustcArtifactPlan, OvenRustcError> {
        self.materialize_trusted_store_composed_with_search_roots(roots, &[], expected_intent)
    }

    /// Bind disjoint canonical artifacts while retaining other admitted copies for closed source search roles.
    ///
    /// Extra search roots carry full publisher inventories under the caller's retained leases. They cannot satisfy
    /// missing canonical artifacts or add extern grants; only exact selected members in a clean directory are used.
    pub fn materialize_trusted_store_composed_with_search_roots(
        &self,
        roots: &[OvenTrustedRustcArtifactRoot<'_>],
        search_roots: &[OvenTrustedRustcSearchRoot<'_>],
        expected_intent: &OvenBuildIntent,
    ) -> Result<OvenRustcArtifactPlan, OvenRustcError> {
        self.validate_shape(expected_intent)?;
        if roots.is_empty() {
            return Err(OvenRustcError::InvalidInput {
                field: "composed artifact roots",
                message: "must contain at least one actively leased foundation".to_string(),
            });
        }
        let expected = expected_artifacts(self)?;
        let mut locations = BTreeMap::<String, PathBuf>::new();
        let mut file_roots = BTreeMap::new();
        let mut inventories = BTreeMap::<(PathBuf, String), BTreeMap<String, String>>::new();
        let mut dependency_search_paths = Vec::new();
        let mut native_search_paths = Vec::new();
        for fragment in roots {
            let root = canonical_directory(fragment.artifact_root, "composed artifact root")?;
            let mut trusted_parents = BTreeMap::new();
            let mut fragment_expected = BTreeMap::new();
            for artifact in fragment.supporting_artifacts {
                let relative = normalized_relative_path(&artifact.relative_path, "composed supporting artifact")?;
                let expected_digest = expected.get(&relative).ok_or_else(|| OvenRustcError::InvalidInput {
                    field: "composed artifact roots",
                    message: format!("declare unrecognized artifact `{relative}`"),
                })?;
                if expected_digest != &artifact.digest {
                    return Err(OvenRustcError::InvalidInput {
                        field: "composed artifact roots",
                        message: format!("declare mismatched digest for `{relative}`"),
                    });
                }
                let path = trusted_file(
                    &root,
                    &relative,
                    "composed supporting artifact",
                    &mut trusted_parents,
                    TrustedShape::Checked,
                )?;
                if locations.insert(relative.clone(), path).is_some() {
                    return Err(OvenRustcError::InvalidInput {
                        field: "composed artifact roots",
                        message: format!("declare duplicate artifact `{relative}`"),
                    });
                }
                file_roots.insert(relative.clone(), root.clone());
                if fragment_expected.insert(relative, artifact.digest.clone()).is_some() {
                    return Err(OvenRustcError::InvalidInput {
                        field: "composed artifact roots",
                        message: "declare one artifact path more than once".to_string(),
                    });
                }
            }
            if fragment_expected.is_empty() {
                return Err(OvenRustcError::InvalidInput {
                    field: "composed artifact roots",
                    message: "must not contain an empty foundation fragment".to_string(),
                });
            }
            dependency_search_paths.extend(trusted_materialize_search_paths(
                &root,
                fragment.dependency_search_paths,
                "composed dependency search",
                &fragment_expected,
                &mut trusted_parents,
                TrustedShape::Checked,
            )?);
            if let Some(root_inventory) = fragment.root_inventory {
                let complete = admitted_search_inventory(root_inventory, &expected)?;
                if fragment_expected
                    .iter()
                    .any(|(path, digest)| complete.get(path) != Some(digest))
                {
                    return Err(OvenRustcError::InvalidInput {
                        field: "composed artifact roots",
                        message: "assigned fragment differs from its complete admitted root inventory".to_string(),
                    });
                }
                for relative in fragment.dependency_search_paths {
                    let directory = (root.clone(), relative.clone());
                    let members: BTreeMap<String, String> = complete
                        .iter()
                        .filter(|(path, _)| artifact_is_below_search_path(path, relative))
                        .map(|(path, digest)| (path.clone(), digest.clone()))
                        .collect();
                    if let Some(previous) = inventories.insert(directory, members.clone())
                        && previous != members
                    {
                        return Err(OvenRustcError::InvalidInput {
                            field: "composed artifact roots",
                            message: "same physical directory has conflicting admitted inventories".to_string(),
                        });
                    }
                }
            } else if !self.entrypoint_dependency_search_paths.is_empty() {
                return Err(OvenRustcError::InvalidInput {
                    field: "composed artifact roots",
                    message: "source isolation requires the complete admitted root inventory".to_string(),
                });
            }
            native_search_paths.extend(trusted_materialize_search_paths(
                &root,
                fragment.native_search_paths,
                "composed native search",
                &fragment_expected,
                &mut trusted_parents,
                TrustedShape::Checked,
            )?);
        }
        let missing = expected
            .keys()
            .filter(|relative| !locations.contains_key(*relative))
            .cloned()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(OvenRustcError::InvalidInput {
                field: "composed artifact roots",
                message: format!("omit required artifact(s): {}", missing.join(", ")),
            });
        }
        for candidate in search_roots {
            let root = canonical_directory(candidate.artifact_root, "admitted search root")?;
            let complete = admitted_search_inventory(candidate.root_inventory, &expected)?;
            let mut trusted_parents = BTreeMap::new();
            trusted_materialize_search_paths(
                &root,
                candidate.dependency_search_paths,
                "admitted dependency search",
                &complete,
                &mut trusted_parents,
                TrustedShape::Checked,
            )?;
            for relative in candidate.dependency_search_paths {
                let members = complete
                    .iter()
                    .filter(|(path, _)| artifact_is_below_search_path(path, relative))
                    .map(|(path, digest)| (path.clone(), digest.clone()))
                    .collect::<BTreeMap<_, _>>();
                if let Some(previous) = inventories.insert((root.clone(), relative.clone()), members.clone())
                    && previous != members
                {
                    return Err(OvenRustcError::InvalidInput {
                        field: "composed artifact roots",
                        message: "same physical directory has conflicting admitted inventories".to_string(),
                    });
                }
            }
        }
        let source_path_projection = self.bind_source_search_roles(&file_roots, &inventories)?;
        if let Some(projection) = &source_path_projection {
            // These paths remain immutable role bindings, not caller grants. Projection removes other roles' paths.
            dependency_search_paths.extend(projection.roles.values().flat_map(|(_, paths)| paths.iter().cloned()));
        }
        dependency_search_paths.sort();
        dependency_search_paths.dedup();
        native_search_paths.sort();
        native_search_paths.dedup();
        let externs = self
            .externs
            .iter()
            .map(|artifact| {
                let relative = normalized_relative_path(&artifact.relative_path, "extern")?;
                let path = locations.get(&relative).ok_or_else(|| OvenRustcError::InvalidInput {
                    field: "composed artifact roots",
                    message: format!("omit extern `{relative}`"),
                })?;
                Ok((artifact.crate_name.clone(), path.clone()))
            })
            .collect::<Result<Vec<_>, OvenRustcError>>()?;
        Ok(OvenRustcArtifactPlan {
            source_path_projection,
            dependency_search_paths,
            native_search_paths,
            externs,
            compile_environment: validated_compile_environment(&self.compile_environment)?,
            caller_owned_library_digests: BTreeMap::new(),
        })
    }
}
