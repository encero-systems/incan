//! Building direct-rustc plans from caller-owned workspace libraries.

use super::{
    BTreeSet, OvenCallerOwnedRustcLibrary, OvenRustcArtifactPlan, OvenRustcError, digest_bytes, fs,
    validate_rust_identifier, verified_regular_file,
};

/// Select only workspace instances reachable from the declared roots, refusing an unrecorded dependency by name.
///
/// The scheduler records the exact feature-unified DAG. Unrequested sibling roots must neither gain extern
/// visibility nor be treated as constituents of the consumer's compiled graph.
pub fn selected_workspace_dependency_closure(
    graph: &super::BTreeMap<String, Vec<String>>,
    roots: &BTreeSet<String>,
) -> Result<BTreeSet<String>, OvenRustcError> {
    let mut selected = BTreeSet::new();
    let mut pending = roots.iter().cloned().collect::<Vec<_>>();
    while let Some(name) = pending.pop() {
        if !selected.insert(name.clone()) {
            continue;
        }
        let dependencies = graph.get(&name).ok_or_else(|| OvenRustcError::InvalidInput {
            field: "compiler workspace cohort",
            message: format!("crate `{name}` has no admitted workspace dependency graph"),
        })?;
        pending.extend(dependencies.iter().cloned());
    }
    Ok(selected)
}

/// Refuse a workspace graph that exposes a second instance through either externs or metadata searches.
///
/// Workspace package names are unique in the admitted compiler graph. Registry packages may have distinct versions,
/// so this check deliberately applies only to the workspace instances the scheduler names. An identical verified
/// artifact may have two physical locations; differing bytes require selecting one owner and rebuilding its consumers.
pub fn validate_selected_workspace_instances(
    plan: &OvenRustcArtifactPlan,
    libraries: &[OvenCallerOwnedRustcLibrary],
) -> Result<(), OvenRustcError> {
    for library in libraries {
        validate_rust_identifier(&library.crate_name)?;
        let expected = verified_regular_file(&library.output, "selected workspace instance")?;
        let actual_digest = super::digest_regular_file(&expected, "selected workspace instance")?;
        if actual_digest != library.digest {
            return Err(OvenRustcError::InvalidInput {
                field: "selected workspace instance",
                message: format!(
                    "crate `{}` changed: expected {}, found {actual_digest}",
                    library.crate_name, library.digest
                ),
            });
        }
        let mut candidates = plan
            .externs
            .iter()
            .filter(|(name, _)| name == &library.crate_name)
            .map(|(_, path)| path.clone())
            .collect::<BTreeSet<_>>();
        let prefix = format!("lib{}-", library.crate_name);
        let plain = format!("lib{}.rlib", library.crate_name);
        for directory in &plan.dependency_search_paths {
            for entry in fs::read_dir(directory).map_err(|source| OvenRustcError::Io {
                path: directory.clone(),
                source,
            })? {
                let entry = entry.map_err(|source| OvenRustcError::Io {
                    path: directory.clone(),
                    source,
                })?;
                let name = entry.file_name();
                let Some(name) = name.to_str() else {
                    continue;
                };
                if name == plain || (name.starts_with(&prefix) && name.ends_with(".rlib")) {
                    candidates.insert(entry.path());
                }
            }
        }
        for candidate in candidates {
            let candidate = verified_regular_file(&candidate, "selected workspace instance")?;
            if candidate == expected {
                continue;
            }
            let candidate_digest = digest_bytes(&fs::read(&candidate).map_err(|source| OvenRustcError::Io {
                path: candidate.clone(),
                source,
            })?);
            if candidate_digest != library.digest {
                return Err(OvenRustcError::InvalidInput {
                    field: "compiler workspace cohort",
                    message: format!(
                        "crate `{}` has two compiled instances: {} at {} and {candidate_digest} at {}; select one owner before compiling any consumer",
                        library.crate_name,
                        library.digest,
                        expected.display(),
                        candidate.display()
                    ),
                });
            }
        }
    }
    Ok(())
}

/// Refuse conflicting target registry artifacts inherited by independently compiled workspace consumers.
///
/// Package version and compilation domain identify comparable units; host tools and different locked versions do
/// not denote one target crate. This conservative check requires byte equality for an overlapping target package,
/// because the suite foundation does not yet retain portable compiled-unit identities that could prove equivalence.
pub fn validate_selected_registry_instances(
    leaves: &[super::OvenRustcRegistryLeaf],
    instances: &[oven_model::compiler_suite_env::OvenCompilerSuiteRustUnitRegistryArtifact],
) -> Result<(), OvenRustcError> {
    for instance in instances.iter().filter(|instance| !instance.host) {
        let actual = super::digest_regular_file(&instance.artifact.output, "suite registry instance")?;
        if actual != instance.artifact.digest {
            return Err(OvenRustcError::InvalidInput {
                field: "suite registry instance",
                message: format!(
                    "crate `{}` changed: expected {}, found {actual}",
                    instance.artifact.crate_name, instance.artifact.digest
                ),
            });
        }
        for leaf in leaves.iter().filter(|leaf| {
            leaf.domain == super::OvenRustcRegistryLeafDomain::Target
                && leaf.package == instance.package
                && leaf.version == instance.version
        }) {
            if leaf.artifact.digest != instance.artifact.digest {
                return Err(OvenRustcError::InvalidInput {
                    field: "compiler registry cohort",
                    message: format!(
                        "crate `{}` ({}@{}) has conflicting artifact identities: sealed {} and suite {} at {}; the compiled consumers must share one foundation",
                        leaf.crate_name,
                        leaf.package,
                        leaf.version,
                        leaf.artifact.digest,
                        instance.artifact.digest,
                        instance.artifact.output.display(),
                    ),
                });
            }
        }
    }
    Ok(())
}

/// Normalize a declared package name to the direct-rustc crate identifier it exposes.
pub(super) fn direct_rustc_crate_name(name: &str) -> Result<String, OvenRustcError> {
    let normalized = name.replace('-', "_");
    validate_rust_identifier(&normalized)?;
    Ok(normalized)
}

/// Add direct-Rustc workspace-library outputs to a previously selected immutable artifact plan.
///
/// The immutable plan continues to own third-party and native inputs. These libraries are the caller-owned bridge
/// between topologically ordered workspace compilation steps, so their already-verified digests are incorporated
/// into the consumer output's reuse identity instead of being mistaken for a Cargo target directory.
pub fn attach_caller_owned_rustc_libraries(
    plan: &mut OvenRustcArtifactPlan,
    libraries: &[OvenCallerOwnedRustcLibrary],
) -> Result<(), OvenRustcError> {
    let mut crate_names = plan
        .externs
        .iter()
        .map(|(name, _)| name.clone())
        .collect::<BTreeSet<_>>();
    for library in libraries {
        validate_rust_identifier(&library.crate_name)?;
        let output = verified_regular_file(&library.output, "caller-owned library")?;
        if library.expose_extern && !crate_names.insert(library.crate_name.clone()) {
            // A package diamond reaches one provider under one alias from two sides (#1459): the consumer names it
            // directly and an intermediate provider names it too. The same sealed artifact arriving twice is not a
            // conflict, so it is attached once; a different artifact under the same name still is one.
            let same_artifact = plan
                .externs
                .iter()
                .any(|(name, path)| name == &library.crate_name && path == &output)
                && plan
                    .caller_owned_library_digests
                    .get(&library.crate_name)
                    .is_none_or(|digest| digest == &library.digest);
            if same_artifact {
                continue;
            }
            return Err(OvenRustcError::InvalidInput {
                field: "caller-owned library",
                message: format!("duplicates direct-Rustc extern `{}`", library.crate_name),
            });
        }
        let extension = output.extension().and_then(|extension| extension.to_str());
        if !matches!(extension, Some("rlib" | "dylib" | "so" | "dll")) {
            return Err(OvenRustcError::InvalidInput {
                field: "caller-owned library",
                message: format!("{} must be a Rust library or procedural-macro output", output.display()),
            });
        }
        let parent = output.parent().ok_or_else(|| OvenRustcError::InvalidInput {
            field: "caller-owned library",
            message: format!("{} has no parent directory", output.display()),
        })?;
        plan.retain_caller_dependency_search_path(parent.to_path_buf());
        let evidence_key = if library.expose_extern {
            library.crate_name.clone()
        } else {
            format!("transitive:{}:{}", library.crate_name, library.digest)
        };
        if plan
            .caller_owned_library_digests
            .insert(evidence_key, library.digest.clone())
            .is_some()
        {
            return Err(OvenRustcError::InvalidInput {
                field: "caller-owned library",
                message: format!("duplicates reuse evidence for `{}`", library.crate_name),
            });
        }
        if library.expose_extern {
            plan.externs.push((library.crate_name.clone(), output));
        }
    }
    plan.dependency_search_paths.sort();
    plan.dependency_search_paths.dedup();
    Ok(())
}
