//! Composing a project extension's artifact plan against the exact release cohort it extends.
//!
//! An extension retains its own locked third-party closure and inherits compiler-owned runtime artifacts, overlapping
//! locked registry units and vocabulary auxiliaries from one immutable base. These methods perform that
//! substitution, validate it, and partition or fragment a plan against its base.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION, OvenRustcArtifactManifest, OvenRustcArtifactPartition,
    OvenRustcAuxiliaryTarget, OvenRustcError, OvenRustcRegistryLeaf, OvenRustcSupportingArtifact, Path,
    artifact_is_below_search_path, canonicalize_release_registry_sources, compiler_runtime_artifacts_by_name,
    compiler_runtime_name_from_artifact_path, compiler_runtime_sidecars_by_name, discard_orphaned_metadata_sidecars,
    expected_artifacts, metadata_sidecar_pair_path, normalized_package_name, registry_leaf_substitution_is_safe,
    replace_compiler_runtime_extern, replace_compiler_runtime_supporting_artifact, replace_declared_release_artifact,
    rerooted_extension_artifact_path, same_registry_leaf_semantics, validate_release_registry_cohort,
};

impl OvenRustcArtifactManifest {
    /// Replace a generated project's release-owned dependency cohort with the exact selected release-base family.
    ///
    /// Cargo gives path dependencies and even locked registry units publisher-local bytes. The selected release Loaf
    /// owns one ABI cohort: exact `incan_*` artifacts plus every registry unit whose package, version, features,
    /// source, target, profile, and toolchain match the sealed release catalog. Project-only registry and path
    /// dependencies remain byte-exact extension inputs. Vocabulary auxiliaries retain their isolated roles.
    ///
    /// `root_registry_packages` names the registry packages the generated root consumes directly (the declared
    /// `[rust-dependencies]`, resolved to Cargo package names). Those leaves may substitute across compilation
    /// identities because the root is recompiled against whatever the composed plan names — that swap is what
    /// unifies the root's trait identities with the sealed standard library. Every other leaf substitutes only onto
    /// an identical compilation identity; see [`registry_leaf_substitution_is_safe`]. When the plan retains
    /// extension-built consumers, no leaf crosses identities, the root's own included: the root then links the
    /// extension's runtime, and its leaves are also dependencies of that runtime and of the retained consumers,
    /// which recorded them by exact identity hash. Because this parameter shapes the composed plan, stored
    /// extension payloads record it and recomposition passes the recorded set.
    pub fn with_release_cohort_from_base(
        &self,
        base: &Self,
        root_registry_packages: &BTreeSet<String>,
    ) -> Result<Self, OvenRustcError> {
        self.validate_shape(&self.intent)?;
        base.validate_shape(&self.intent)?;
        validate_release_registry_cohort(self, base)?;
        let project_runtime_indexes = self
            .externs
            .iter()
            .enumerate()
            .filter_map(|(index, artifact)| (artifact.crate_name == "incan_std_core").then_some(index))
            .collect::<Vec<_>>();
        let [project_runtime_index] = project_runtime_indexes.as_slice() else {
            return Err(OvenRustcError::InvalidInput {
                field: "project extension runtime",
                message: "must declare exactly one `incan_std_core` root extern".to_string(),
            });
        };
        let base_runtimes = base
            .externs
            .iter()
            .filter(|artifact| artifact.crate_name == "incan_std_core")
            .collect::<Vec<_>>();
        let [base_runtime] = base_runtimes.as_slice() else {
            return Err(OvenRustcError::InvalidInput {
                field: "project extension base",
                message: "must declare exactly one `incan_std_core` root extern".to_string(),
            });
        };
        let mut composed = self.clone();
        if self.schema_version != base.schema_version
            || self.schema_version == OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION
        {
            composed.schema_version = OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION;
            if composed.entrypoint_externs.is_empty() {
                composed.entrypoint_externs.insert(
                    "generated-root".to_string(),
                    self.externs
                        .iter()
                        .map(|artifact| artifact.crate_name.clone())
                        .collect(),
                );
            }
            composed.entrypoint_dependency_search_paths = composed
                .entrypoint_externs
                .keys()
                .map(|key| Ok((key.clone(), self.source_search_closure(key)?)))
                .collect::<Result<_, OvenRustcError>>()?;
        }
        let root_registry_packages = root_registry_packages
            .iter()
            .map(|package| normalized_package_name(package))
            .collect::<BTreeSet<_>>();
        let root_extern_crates = composed
            .externs
            .iter()
            .map(|artifact| artifact.crate_name.clone())
            .collect::<BTreeSet<_>>();
        // Cross-identity substitution is only safe for a leaf whose extension identity nothing retained records
        // (#1227). Two structural facts force the conservative regime for the whole plan: a root-linked leaf with no
        // semantics-matching release counterpart (Bevy, DataFusion) keeps its extension-built subtree, and a
        // prebuilt project crate outside the leaf set (a workspace path dependency) keeps its recorded dependencies;
        // in both cases retained prebuilt crates hold exact identity hashes of other leaves. Without either fact,
        // every retained consumer is recompiled against the composed plan — the generated root plus the sealed
        // release family — and swapping shared leaves onto the release copies is exactly what unifies the root's
        // trait identities with the standard library. In the conservative regime nothing crosses identities, the
        // root's own leaves included: the root links the extension's runtime (below), and a root-linked leaf such
        // as `serde` is also an ordinary dependency of the retained consumers and of that runtime, so the unified
        // Cargo resolution stays the plan and the base contributes only byte-identical canonicalization.
        let leaf_is_root_linked = |leaf: &OvenRustcRegistryLeaf| {
            root_extern_crates.contains(&leaf.crate_name)
                || root_registry_packages.contains(&normalized_package_name(&leaf.package))
        };
        let leaf_has_release_counterpart = |leaf: &OvenRustcRegistryLeaf| {
            base.registry_leaves
                .iter()
                .any(|candidate| same_registry_leaf_semantics(candidate, leaf))
        };
        let leaf_artifact_paths = composed
            .registry_leaves
            .iter()
            .map(|leaf| leaf.artifact.relative_path.clone())
            .collect::<BTreeSet<_>>();
        let has_project_prebuilt_supporting = composed.supporting_artifacts.iter().any(|artifact| {
            artifact.relative_path.ends_with(".rlib")
                && !leaf_artifact_paths.contains(&artifact.relative_path)
                && compiler_runtime_name_from_artifact_path(&artifact.relative_path).is_none()
        });
        let retains_extension_built_consumers = has_project_prebuilt_supporting
            || composed
                .registry_leaves
                .iter()
                .any(|leaf| leaf_is_root_linked(leaf) && !leaf_has_release_counterpart(leaf));
        // ---- Select the compiler runtime the root links ----
        // Every consumer recompiled against the composed plan links the sealed release runtime, which is what
        // unifies the root's trait identities with the standard library. The conservative regime cannot: its
        // retained consumers were compiled by one unified Cargo resolution together with the project's own copy
        // of the runtime, and a unit with process-global state — an async runtime above all — must be linked once.
        // Taking the base's runtime there would bring the base's `tokio` in through the standard library's async
        // component while DataFusion keeps the extension's, and the reactor one side starts is invisible to the
        // other. So the root keeps the extension's runtime in that regime, and the whole link stays the single
        // resolution Cargo produced. The base still contributes its release execution artifacts and vocabulary
        // auxiliaries below.
        if !retains_extension_built_consumers {
            let mut replacements = Vec::new();
            let original_runtime = &composed.externs[*project_runtime_index];
            replacements.push((
                OvenRustcSupportingArtifact {
                    relative_path: original_runtime.relative_path.clone(),
                    digest: original_runtime.digest.clone(),
                },
                Some(OvenRustcSupportingArtifact {
                    relative_path: base_runtime.relative_path.clone(),
                    digest: base_runtime.digest.clone(),
                }),
            ));
            composed.externs[*project_runtime_index] = (*base_runtime).clone();
            let base_compiler_artifacts = compiler_runtime_artifacts_by_name(base)?;
            let base_compiler_sidecars = compiler_runtime_sidecars_by_name(base)?;
            for artifact in &mut composed.externs {
                let original = OvenRustcSupportingArtifact {
                    relative_path: artifact.relative_path.clone(),
                    digest: artifact.digest.clone(),
                };
                replace_compiler_runtime_extern(artifact, &base_compiler_artifacts)?;
                replacements.push((
                    original,
                    Some(OvenRustcSupportingArtifact {
                        relative_path: artifact.relative_path.clone(),
                        digest: artifact.digest.clone(),
                    }),
                ));
            }
            composed.supporting_artifacts = std::mem::take(&mut composed.supporting_artifacts)
                .into_iter()
                .filter_map(|artifact| {
                    let original = artifact.clone();
                    let replacement = replace_compiler_runtime_supporting_artifact(
                        artifact,
                        &base_compiler_artifacts,
                        &base_compiler_sidecars,
                    );
                    replacements.push((original, replacement.clone()));
                    replacement
                })
                .collect();
            for (original, replacement) in replacements {
                for closure in composed.entrypoint_dependency_search_paths.values_mut() {
                    closure.replace_artifact(&original, replacement.as_ref());
                }
            }
        }
        canonicalize_release_registry_sources(&mut composed, base)?;
        let mut leaf_replacements = Vec::new();
        for (index, project_leaf) in composed.registry_leaves.iter().enumerate() {
            let candidates = base
                .registry_leaves
                .iter()
                .filter(|candidate| {
                    candidate.package == project_leaf.package
                        && candidate.source.registry == project_leaf.source.registry
                })
                .collect::<Vec<_>>();
            if candidates.is_empty() {
                continue;
            }
            let allow_cross_identity = !retains_extension_built_consumers;
            let Some(release_leaf) = candidates.into_iter().find(|candidate| {
                same_registry_leaf_semantics(candidate, project_leaf)
                    && registry_leaf_substitution_is_safe(project_leaf, candidate, allow_cross_identity)
                    && composed.registry_sources.iter().any(|project_source| {
                        base.registry_sources.iter().any(|release_source| {
                            project_source == release_source
                                && release_source.package == candidate.package
                                && release_source.version == candidate.version
                                && release_source.source == candidate.source
                        })
                    })
            }) else {
                continue;
            };
            leaf_replacements.push((
                index,
                project_leaf.artifact.relative_path.clone(),
                (*release_leaf).clone(),
            ));
        }
        for (index, project_path, release_leaf) in leaf_replacements {
            replace_declared_release_artifact(&mut composed, &project_path, &release_leaf.artifact);
            composed.registry_leaves[index] = release_leaf;
        }
        // ---- Reconcile extension artifacts that collide with the base execution closure ----
        // A composed artifact can share its filename — and therefore its relative path — with the base's execution
        // closure while holding different bytes: Cargo's extra-filename summarizes declared unit inputs, not the
        // build environment, so a base prebuilt on another machine publishes the same filename with a different
        // strict version hash. Which copy the final link needs depends on the regime. When every retained consumer
        // is recompiled against the composed plan, the extension's publisher-local bytes are superseded and each
        // colliding record adopts the base copy. When prebuilt extension crates are retained, they recorded the
        // local hashes — including proc-macro dependencies — so the link needs both copies: the extension's moves
        // into the sibling `extension-deps` directory with its project digest intact, its split-metadata partner
        // moves with it so neither half is stranded, and the base's copy joins the plan untouched below. A
        // conservative-regime collision outside a `deps` directory has no re-rooted home and stays a hard
        // incompatibility, reported by the merge below.
        let release_artifacts = base
            .release_execution_artifacts()?
            .into_iter()
            .map(|artifact| (artifact.relative_path.clone(), artifact))
            .collect::<BTreeMap<_, _>>();
        let mut source_replacements = Vec::new();
        if retains_extension_built_consumers {
            let declared_paths = composed
                .externs
                .iter()
                .map(|artifact| (artifact.relative_path.clone(), artifact.digest.clone()))
                .chain(
                    composed
                        .supporting_artifacts
                        .iter()
                        .map(|artifact| (artifact.relative_path.clone(), artifact.digest.clone())),
                )
                .collect::<BTreeMap<_, _>>();
            // A salted extension unit shares its Cargo filename with the base's twin while carrying a distinct
            // StableCrateId, so the plan needs both files: the extension's copy moves into `extension-deps` with
            // its split-metadata sidecar, the directory joins the search paths, and the base's copy joins the plan
            // untouched below. rustc then selects each dependent's copy by the exact hash it recorded. Only a
            // colliding artifact outside a `deps` directory has no re-rooted home; that remains a hard
            // incompatibility, reported by the merge below.
            // A colliding artifact re-roots only when the plan retained the project's compilation of its whole
            // unit. When the unit's linkable half already follows the release — a root-linked leaf substituted onto
            // the base copy — the colliding metadata sidecar describes the base's compilation and adopts the base
            // digest instead of moving: dragging the substituted record into `extension-deps` would pair a
            // base-digest record with salted project bytes.
            let partner_follows_the_base = |relative_path: &str| {
                metadata_sidecar_pair_path(relative_path).is_some_and(|partner| {
                    declared_paths.get(&partner).is_some_and(|partner_digest| {
                        release_artifacts
                            .get(&partner)
                            .is_some_and(|release| &release.digest == partner_digest)
                    })
                })
            };
            let mut rerooted_paths = BTreeMap::new();
            let mut base_adopted_paths = BTreeSet::new();
            for (relative_path, digest) in &declared_paths {
                let Some(release) = release_artifacts.get(relative_path) else {
                    continue;
                };
                if &release.digest == digest {
                    continue;
                }
                if partner_follows_the_base(relative_path) {
                    base_adopted_paths.insert(relative_path.clone());
                    continue;
                }
                let Some(rerooted) = rerooted_extension_artifact_path(relative_path) else {
                    continue;
                };
                rerooted_paths.insert(relative_path.clone(), rerooted);
            }
            for (relative_path, rerooted) in rerooted_paths.clone() {
                let Some(partner) = metadata_sidecar_pair_path(&relative_path) else {
                    continue;
                };
                if !declared_paths.contains_key(&partner)
                    || rerooted_paths.contains_key(&partner)
                    || base_adopted_paths.contains(&partner)
                {
                    continue;
                }
                let Some(partner_rerooted) = metadata_sidecar_pair_path(&rerooted) else {
                    continue;
                };
                rerooted_paths.insert(partner, partner_rerooted);
            }
            for artifact in &mut composed.externs {
                let original = OvenRustcSupportingArtifact {
                    relative_path: artifact.relative_path.clone(),
                    digest: artifact.digest.clone(),
                };
                if base_adopted_paths.contains(&artifact.relative_path)
                    && let Some(release) = release_artifacts.get(&artifact.relative_path)
                {
                    artifact.digest = release.digest.clone();
                }
                source_replacements.push((
                    original,
                    OvenRustcSupportingArtifact {
                        relative_path: artifact.relative_path.clone(),
                        digest: artifact.digest.clone(),
                    },
                ));
            }
            for artifact in &mut composed.supporting_artifacts {
                let original = OvenRustcSupportingArtifact {
                    relative_path: artifact.relative_path.clone(),
                    digest: artifact.digest.clone(),
                };
                if base_adopted_paths.contains(&artifact.relative_path)
                    && let Some(release) = release_artifacts.get(&artifact.relative_path)
                {
                    artifact.digest = release.digest.clone();
                }
                source_replacements.push((
                    original,
                    OvenRustcSupportingArtifact {
                        relative_path: artifact.relative_path.clone(),
                        digest: artifact.digest.clone(),
                    },
                ));
            }
            for leaf in &mut composed.registry_leaves {
                if base_adopted_paths.contains(&leaf.artifact.relative_path)
                    && let Some(release) = release_artifacts.get(&leaf.artifact.relative_path)
                {
                    leaf.artifact.digest = release.digest.clone();
                }
            }
            if !rerooted_paths.is_empty() {
                // A record that already carries the release digest at a colliding path is the base's copy and
                // stays put; only the project's copy under that path moves.
                let follows_the_base = |relative_path: &str, digest: &str| {
                    release_artifacts
                        .get(relative_path)
                        .is_some_and(|release| release.digest == digest)
                };
                for artifact in &mut composed.externs {
                    let original = OvenRustcSupportingArtifact {
                        relative_path: artifact.relative_path.clone(),
                        digest: artifact.digest.clone(),
                    };
                    if let Some(rerooted) = rerooted_paths.get(&artifact.relative_path)
                        && !follows_the_base(&artifact.relative_path, &artifact.digest)
                    {
                        artifact.relative_path = rerooted.clone();
                    }
                    source_replacements.push((
                        original,
                        OvenRustcSupportingArtifact {
                            relative_path: artifact.relative_path.clone(),
                            digest: artifact.digest.clone(),
                        },
                    ));
                }
                for artifact in &mut composed.supporting_artifacts {
                    let original = OvenRustcSupportingArtifact {
                        relative_path: artifact.relative_path.clone(),
                        digest: artifact.digest.clone(),
                    };
                    if let Some(rerooted) = rerooted_paths.get(&artifact.relative_path)
                        && !follows_the_base(&artifact.relative_path, &artifact.digest)
                    {
                        artifact.relative_path = rerooted.clone();
                    }
                    source_replacements.push((
                        original,
                        OvenRustcSupportingArtifact {
                            relative_path: artifact.relative_path.clone(),
                            digest: artifact.digest.clone(),
                        },
                    ));
                }
                for leaf in &mut composed.registry_leaves {
                    if let Some(rerooted) = rerooted_paths.get(&leaf.artifact.relative_path)
                        && !follows_the_base(&leaf.artifact.relative_path, &leaf.artifact.digest)
                    {
                        leaf.artifact.relative_path = rerooted.clone();
                    }
                }
                let mut rerooted_search_dirs = BTreeSet::new();
                for rerooted in rerooted_paths.values() {
                    if let Some(parent) = Path::new(rerooted).parent()
                        && !parent.as_os_str().is_empty()
                    {
                        rerooted_search_dirs.insert(parent.to_string_lossy().into_owned());
                    }
                }
                composed.dependency_search_paths.extend(rerooted_search_dirs);
            }
        } else {
            for artifact in &mut composed.externs {
                let original = OvenRustcSupportingArtifact {
                    relative_path: artifact.relative_path.clone(),
                    digest: artifact.digest.clone(),
                };
                if let Some(release) = release_artifacts.get(&artifact.relative_path) {
                    artifact.digest = release.digest.clone();
                }
                source_replacements.push((
                    original,
                    OvenRustcSupportingArtifact {
                        relative_path: artifact.relative_path.clone(),
                        digest: artifact.digest.clone(),
                    },
                ));
            }
            for artifact in &mut composed.supporting_artifacts {
                let original = OvenRustcSupportingArtifact {
                    relative_path: artifact.relative_path.clone(),
                    digest: artifact.digest.clone(),
                };
                if let Some(release) = release_artifacts.get(&artifact.relative_path) {
                    artifact.digest = release.digest.clone();
                }
                source_replacements.push((
                    original,
                    OvenRustcSupportingArtifact {
                        relative_path: artifact.relative_path.clone(),
                        digest: artifact.digest.clone(),
                    },
                ));
            }
            for leaf in &mut composed.registry_leaves {
                if let Some(release) = release_artifacts.get(&leaf.artifact.relative_path) {
                    leaf.artifact.digest = release.digest.clone();
                }
            }
        }
        for (original, replacement) in source_replacements {
            for closure in composed.entrypoint_dependency_search_paths.values_mut() {
                closure.replace_artifact(&original, Some(&replacement));
            }
        }
        composed.vocab_auxiliary_targets = base.vocab_auxiliary_targets.clone();
        let mut declared = expected_artifacts(&composed)?;
        for artifact in base.release_execution_artifacts()? {
            if artifact.relative_path == base_runtime.relative_path {
                continue;
            }
            match declared.get(&artifact.relative_path) {
                Some(digest) if digest == &artifact.digest => {}
                Some(digest) => {
                    return Err(OvenRustcError::InvalidInput {
                        field: "project extension release support",
                        message: format!(
                            "release-base artifact `{}` conflicts with project artifact digest {digest}",
                            artifact.relative_path
                        ),
                    });
                }
                None => {
                    declared.insert(artifact.relative_path.clone(), artifact.digest.clone());
                    composed.supporting_artifacts.push(artifact);
                }
            }
        }
        composed
            .dependency_search_paths
            .extend(base.dependency_search_paths.iter().cloned());
        composed.dependency_search_paths.sort();
        composed.dependency_search_paths.dedup();
        for (key, closure) in &mut composed.entrypoint_dependency_search_paths {
            let base_key = if base.entrypoint_externs.contains_key(key) {
                key.as_str()
            } else {
                "generated-root"
            };
            closure.merge(&base.source_search_closure(base_key)?);
        }
        // The base's own `incan_std_core` root is the one release artifact the loop above deliberately skips: the
        // extension links its own runtime instead. A closure merged from the base still carries it as a member of
        // the search directory, and a member the composed manifest never declares is exactly what
        // `validate_source_search_roles` refuses. Drop that stale claim -- and only when it really is stale, since
        // a canonicalizing composition may legitimately keep the base's bytes at the same path.
        //
        // The question is answered by scanning the three declaring lists rather than through `expected_artifacts`,
        // which additionally refuses a duplicate path. The composition is still mid-flight here: `validate_shape`
        // below is where a genuine duplicate must be reported, and taking that judgement early turned a transient
        // arrangement into a bake failure.
        let declares_base_runtime = composed
            .externs
            .iter()
            .chain(composed.registry_leaves.iter().map(|leaf| &leaf.artifact))
            .any(|artifact| {
                artifact.relative_path == base_runtime.relative_path && artifact.digest == base_runtime.digest
            })
            || composed.supporting_artifacts.iter().any(|artifact| {
                artifact.relative_path == base_runtime.relative_path && artifact.digest == base_runtime.digest
            });
        if !declares_base_runtime {
            let stale = OvenRustcSupportingArtifact {
                relative_path: base_runtime.relative_path.clone(),
                digest: base_runtime.digest.clone(),
            };
            for closure in composed.entrypoint_dependency_search_paths.values_mut() {
                closure.replace_artifact(&stale, None);
            }
        }
        composed
            .native_search_paths
            .extend(base.native_search_paths.iter().cloned());
        composed.native_search_paths.sort();
        composed.native_search_paths.dedup();
        discard_orphaned_metadata_sidecars(&mut composed);
        composed.supporting_artifacts.sort_by(|left, right| {
            left.relative_path
                .cmp(&right.relative_path)
                .then_with(|| left.digest.cmp(&right.digest))
        });
        composed.validate_shape(&self.intent)?;
        Ok(composed)
    }

    /// Verify that this project plan already uses the exact release cohort selected from its base.
    ///
    /// An already-composed plan recomposes to itself regardless of the root-package set: leaves that were
    /// substituted are identity-equal to the base copy, and leaves that were kept decline substitution again, so
    /// this check passes an empty root set.
    pub fn validate_release_cohort_from_base(&self, base: &Self) -> Result<(), OvenRustcError> {
        if self.with_release_cohort_from_base(base, &BTreeSet::new())? != *self {
            return Err(OvenRustcError::InvalidInput {
                field: "project extension release cohort",
                message:
                    "does not use the exact runtime, registry, and vocabulary cohort from its selected release base"
                        .to_string(),
            });
        }
        Ok(())
    }

    /// Partition this complete publisher closure against one already selected base Loaf.
    ///
    /// Only byte-identical declared artifacts can be supplied by the base.  A path collision with a different
    /// digest is a feature/toolchain incompatibility, not an opportunity to select whichever copy happens to be
    /// present first.  The caller retains the two resulting fragments under independent active leases and composes
    /// them through [`Self::materialize_trusted_store_composed`].
    pub fn partition_against_base(&self, base: &Self) -> Result<OvenRustcArtifactPartition, OvenRustcError> {
        self.validate_shape(&self.intent)?;
        base.validate_shape(&self.intent)?;
        let complete = expected_artifacts(self)?;
        let base_artifacts = expected_artifacts(base)?;
        let mut base_paths = BTreeSet::new();
        let mut extension_paths = BTreeSet::new();
        for (path, digest) in complete {
            match base_artifacts.get(&path) {
                Some(base_digest) if base_digest == &digest => {
                    base_paths.insert(path);
                }
                Some(base_digest) => {
                    return Err(OvenRustcError::InvalidInput {
                        field: "project extension base",
                        message: format!(
                            "artifact `{path}` conflicts with selected base Loaf digest {base_digest}; project extension requires {digest}"
                        ),
                    });
                }
                None => {
                    extension_paths.insert(path);
                }
            }
        }
        Ok(OvenRustcArtifactPartition {
            base_paths,
            extension_paths,
        })
    }

    /// Retain the one verified fragment of this complete closure whose artifacts are named by `paths`.
    ///
    /// The result deliberately omits source-evidence routing: only the complete outer manifest owns the execution
    /// contract.  A fragment is an artifact-root declaration for the compositor, not a second executable plan.
    pub fn artifact_fragment(&self, paths: &BTreeSet<String>) -> Result<Self, OvenRustcError> {
        self.validate_shape(&self.intent)?;
        let declared = expected_artifacts(self)?;
        if let Some(unknown) = paths.iter().find(|path| !declared.contains_key(*path)) {
            return Err(OvenRustcError::InvalidInput {
                field: "project extension fragment",
                message: format!("names artifact `{unknown}` absent from the complete plan"),
            });
        }
        let includes = |relative_path: &str| paths.contains(relative_path);
        let retains_search_path = |search_path: &str| {
            paths
                .iter()
                .any(|relative_path| artifact_is_below_search_path(relative_path, search_path))
        };
        let mut vocab_auxiliary_targets = Vec::new();
        for auxiliary in &self.vocab_auxiliary_targets {
            let externs = auxiliary
                .externs
                .iter()
                .filter(|artifact| includes(&artifact.relative_path))
                .cloned()
                .collect::<Vec<_>>();
            if !externs.is_empty() {
                vocab_auxiliary_targets.push(OvenRustcAuxiliaryTarget {
                    target: auxiliary.target.clone(),
                    dependency_search_paths: auxiliary
                        .dependency_search_paths
                        .iter()
                        .filter(|path| retains_search_path(path))
                        .cloned()
                        .collect(),
                    externs,
                });
            }
        }
        Ok(Self {
            schema_version: self.schema_version,
            intent: self.intent.clone(),
            dependency_search_paths: self
                .dependency_search_paths
                .iter()
                .filter(|path| retains_search_path(path))
                .cloned()
                .collect(),
            native_search_paths: self
                .native_search_paths
                .iter()
                .filter(|path| retains_search_path(path))
                .cloned()
                .collect(),
            externs: self
                .externs
                .iter()
                .filter(|artifact| includes(&artifact.relative_path))
                .cloned()
                .collect(),
            entrypoint_dependency_search_paths: Default::default(),
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: Vec::new(),
            registry_sources: Vec::new(),
            compile_environment: self.compile_environment.clone(),
            vocab_auxiliary_targets,
            supporting_artifacts: self
                .supporting_artifacts
                .iter()
                .filter(|artifact| includes(&artifact.relative_path))
                .cloned()
                .collect(),
        })
    }

    /// Return every declared artifact as a root fragment record for trusted multi-root composition.
    ///
    /// `externs` and vocabulary auxiliary externs are execution roles, not separate files.  The compositor needs
    /// every physical path exactly once before it can later map the complete outer manifest's `--extern` list.
    pub fn composition_artifacts(&self) -> Result<Vec<OvenRustcSupportingArtifact>, OvenRustcError> {
        self.validate_shape(&self.intent)?;
        let mut artifacts = self
            .externs
            .iter()
            .map(|artifact| OvenRustcSupportingArtifact {
                relative_path: artifact.relative_path.clone(),
                digest: artifact.digest.clone(),
            })
            .chain(self.supporting_artifacts.iter().cloned())
            .chain(self.vocab_auxiliary_targets.iter().flat_map(|auxiliary| {
                auxiliary.externs.iter().map(|artifact| OvenRustcSupportingArtifact {
                    relative_path: artifact.relative_path.clone(),
                    digest: artifact.digest.clone(),
                })
            }))
            .collect::<Vec<_>>();
        artifacts.sort_by(|left, right| {
            left.relative_path
                .cmp(&right.relative_path)
                .then_with(|| left.digest.cmp(&right.digest))
        });
        if artifacts
            .windows(2)
            .any(|pair| pair[0].relative_path == pair[1].relative_path)
        {
            return Err(OvenRustcError::InvalidInput {
                field: "artifact manifest",
                message: "declares one composition artifact path more than once".to_string(),
            });
        }
        Ok(artifacts)
    }
}
