//! Building direct-rustc plans from caller-owned workspace libraries.

use super::{
    BTreeSet, OvenCallerOwnedRustcLibrary, OvenRustcArtifactPlan, OvenRustcError, validate_rust_identifier,
    verified_regular_file,
};

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
