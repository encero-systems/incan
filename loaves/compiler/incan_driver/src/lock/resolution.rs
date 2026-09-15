//! Resolving the lock context a command runs under and publishing a project's semantic lock.
//!
//! Manifest-less single-file builds retain their caller context and have no lock payload. Manifest-backed builds use
//! the semantic Incan lock as their authority; a missing non-strict lock is published without constructing a Cargo
//! project, and a stale lock is never reused as an authority.

use std::fs;
use std::path::{Path, PathBuf};

use crate::cargo_policy::{CargoPolicy, enforce_project_toolchain_constraint};
use crate::error::{CliError, CliResult};
use crate::lock::workspace::{
    collect_project_lock_context, collect_workspace_lock_context, collect_workspace_lock_context_tolerating,
};
use crate::lock::{
    CargoLockAuthority, INERT_CARGO_LOCK_PAYLOAD, LockResolution, LockResolutionRequest, OvenLockValidationRequest,
    ProjectLockContext, ProviderBakeLockPublication, PublishedOvenProjectLock, WorkspaceLockResolutionRequest,
};
use crate::session::CompilationSession;
use incan_provider::dependency_resolver::ResolvedDependencies;
use incan_provider::requirements::{
    ProjectRequirements, merge_project_requirement_dependencies, semantic_sdk_path_dependencies,
};
use incan_provider::sdk_store::INTERNAL_CARGO_LOCK_PAYLOAD_PATH_ENV;
use incan_provider::{FeatureSelection, SDK_PROVIDER_BUILD_ENV};
use oven_model::lock::{
    CargoFeatureSelection, IncanLock, LOCK_FILENAME, PublicationLock, SemanticLockState,
    compute_resolved_fingerprint_with_sdk_paths,
};
use oven_model::manifest::ProjectManifest;
use oven_model::workspace::WorkspaceGraph;

/// Collect and publish the one canonical project or workspace lock, retaining the exact immutable inputs for its
/// explicit Oven publisher.
pub fn collect_and_publish_project_lock(
    manifest: &ProjectManifest,
    entry_file: Option<&Path>,
    cargo_features: &CargoFeatureSelection,
    package_features: &FeatureSelection,
    sdk_profile_override: Option<&str>,
) -> CliResult<ProjectLockContext> {
    if let Some(workspace) =
        WorkspaceGraph::discover(manifest.project_root()).map_err(|error| CliError::failure(error.to_string()))?
    {
        let lock_path = workspace.root().join(LOCK_FILENAME);
        let publication_lock = oven_model::lock::acquire_publication_lock(&lock_path).map_err(|error| {
            CliError::failure(format!("failed to acquire workspace lock publication guard: {error}"))
        })?;
        let context = collect_workspace_lock_context(
            &workspace,
            manifest.project_root(),
            entry_file,
            cargo_features,
            sdk_profile_override,
            None,
        )?;
        generate_oven_lockfile(
            workspace.root(),
            &context.resolved,
            &context.project_requirements,
            cargo_features,
            &context.semantic,
            Some(&publication_lock),
        )?;
        return Ok(context);
    }

    let context = collect_project_lock_context(
        manifest,
        entry_file,
        cargo_features,
        package_features,
        sdk_profile_override,
        None,
        None,
    )?
    .ok_or_else(|| CliError::failure("incan lock requires a FILE argument or at least one [project.scripts] entry"))?;
    generate_oven_lockfile(
        manifest.project_root(),
        &context.resolved,
        &context.project_requirements,
        cargo_features,
        &context.semantic,
        None,
    )?;
    Ok(context)
}

/// Read the compiler-owned Cargo.lock payload override used while building an internal artifact.
fn cargo_lock_payload_override(path: Option<PathBuf>) -> CliResult<Option<String>> {
    let Some(path) = path else {
        return Ok(None);
    };
    let payload = fs::read_to_string(&path).map_err(|error| {
        CliError::failure(format!(
            "failed to read internal Cargo.lock payload override {}: {error}",
            path.display()
        ))
    })?;
    Ok(Some(oven_model::lock::normalize_cargo_lock_payload(&payload)))
}

/// Resolve the canonical Oven lock context that normal generated-project callers consume as one unit.
///
/// Manifest-less single-file builds retain their caller context and have no lock payload. Manifest-backed builds use
/// the semantic Incan lock as their authority: a missing non-strict lock is published without constructing a Cargo
/// project, while a stale lock is never reused as an authority. The explicit `legacy_cargo` publisher owns the
/// separate historical Cargo-resolution boundary.
pub fn resolve_lock_context(request: LockResolutionRequest<'_>) -> CliResult<LockResolution> {
    let LockResolutionRequest {
        project_root,
        project_name,
        entry_file,
        manifest,
        resolved,
        project_requirements,
        cargo_features,
        cargo_policy,
        semantic,
        package_features,
        sdk_profile_override,
    } = request;

    let mut caller_resolved = resolved.clone();
    merge_project_requirement_dependencies(&mut caller_resolved, project_requirements)?;

    if manifest.is_none() {
        return Ok(LockResolution {
            cargo_lock_authority: CargoLockAuthority::None,
            cargo_package_name: project_name.to_string(),
            resolved: caller_resolved,
            project_requirements: project_requirements.clone(),
        });
    }

    if std::env::var_os(SDK_PROVIDER_BUILD_ENV).is_some()
        && let Some(payload) = cargo_lock_payload_override(
            std::env::var_os(INTERNAL_CARGO_LOCK_PAYLOAD_PATH_ENV)
                .filter(|path| !path.is_empty())
                .map(PathBuf::from),
        )?
    {
        return Ok(LockResolution {
            cargo_lock_authority: CargoLockAuthority::Exact { payload },
            cargo_package_name: project_name.to_string(),
            resolved: caller_resolved,
            project_requirements: project_requirements.clone(),
        });
    }

    let default_package_features = FeatureSelection::default();
    if let Some(manifest) = manifest
        && let Some(workspace) =
            WorkspaceGraph::discover(manifest.project_root()).map_err(|error| CliError::failure(error.to_string()))?
    {
        return resolve_workspace_lock_payload(WorkspaceLockResolutionRequest {
            workspace: &workspace,
            caller_project_name: project_name,
            caller_root: manifest.project_root(),
            caller_resolved: &caller_resolved,
            caller_project_requirements: project_requirements,
            caller_entry_file: entry_file,
            cargo_features,
            cargo_policy,
            sdk_profile_override,
        });
    }
    let project_context = if let Some(manifest) = manifest {
        collect_project_lock_context(
            manifest,
            entry_file,
            cargo_features,
            package_features.unwrap_or(&default_package_features),
            sdk_profile_override,
            None,
            None,
        )?
    } else {
        None
    };
    let (canonical_resolved, canonical_project_requirements) = if let Some(context) = project_context.as_ref() {
        (context.resolved.clone(), context.project_requirements.clone())
    } else {
        (caller_resolved.clone(), project_requirements.clone())
    };
    let lock_path = project_root.join(LOCK_FILENAME);
    let mut canonical_resolved_with_requirements = canonical_resolved;
    merge_project_requirement_dependencies(
        &mut canonical_resolved_with_requirements,
        &canonical_project_requirements,
    )?;
    // A manifest-backed lock is project-wide, so its canonical context must win over an entrypoint-local semantic
    // snapshot. The supplied snapshot remains authoritative for manifest-less callers, where no project closure can
    // be rebuilt.
    let semantic = project_context
        .as_ref()
        .map(|context| context.semantic.clone())
        .or_else(|| semantic.cloned())
        .unwrap_or_default();
    let semantic_sdk_paths = semantic_sdk_path_dependencies(&canonical_project_requirements);
    let fingerprint = compute_resolved_fingerprint_with_sdk_paths(
        &canonical_resolved_with_requirements.dependencies,
        &canonical_resolved_with_requirements.dev_dependencies,
        cargo_features,
        Some(project_root),
        &semantic,
        &semantic_sdk_paths,
    );

    let strict = cargo_policy.locked || cargo_policy.frozen;
    if strict && let Some(message) = strict_git_source_error(&canonical_resolved_with_requirements) {
        return Err(CliError::failure(message));
    }
    if lock_path.exists() {
        let lock = IncanLock::load(&lock_path).map_err(|e| CliError::failure(e.to_string()))?;
        if lock.deps_fingerprint != fingerprint {
            if strict {
                return Err(CliError::failure(format!(
                    "oven.lock is out of date\n\n\
                     \x20 expected deps-fingerprint: {fingerprint}\n\
                     \x20   actual deps-fingerprint: {actual}\n\n\
                     This usually means your dependency inputs changed since the lock was generated:\n\n\
                     \x20 - loaf.toml dependency entries changed, and/or\n\
                     \x20 - inline rust::... annotations changed, and/or\n\
                     \x20 - toolchain known-good defaults changed (if you rely on defaults)\n\
                     \x20 - Incan package-feature or SDK-profile selection changed, and/or\n\
                     \x20 - Cargo feature selection changed\n\n\
                     Fix:\n\n\
                     \x20   incan lock\n\n\
                     Tip: Pin crate versions/features explicitly in loaf.toml for stability \
                     across toolchain upgrades.",
                    actual = lock.deps_fingerprint,
                )));
            }
            eprintln!(
                "warning: oven.lock is out of date; continuing without using it as Oven lock authority or \
                 rewriting it. Run `incan lock` to refresh it."
            );
            return Ok(LockResolution {
                cargo_lock_authority: CargoLockAuthority::Stale,
                cargo_package_name: project_name.to_string(),
                resolved: caller_resolved,
                project_requirements: project_requirements.clone(),
            });
        }
        return Ok(LockResolution {
            // Normal Oven execution must not materialize a generated Cargo.lock from the compatibility payload
            // retained in oven.lock. The payload is inert for this route; the semantic fingerprint above is the
            // authority that was just verified.
            cargo_lock_authority: CargoLockAuthority::None,
            cargo_package_name: project_name.to_string(),
            resolved: caller_resolved,
            project_requirements: project_requirements.clone(),
        });
    }

    if strict {
        return Err(CliError::failure("oven.lock is missing; run `incan lock`".to_string()));
    }

    generate_oven_lockfile(
        project_root,
        &canonical_resolved_with_requirements,
        &canonical_project_requirements,
        cargo_features,
        &semantic,
        None,
    )?;
    Ok(LockResolution {
        cargo_lock_authority: CargoLockAuthority::None,
        cargo_package_name: project_name.to_string(),
        resolved: caller_resolved,
        project_requirements: project_requirements.clone(),
    })
}

/// Validate strict Oven lock policy without a pre-existing command compilation session.
pub fn validate_oven_lock_policy(
    project_root: &Path,
    manifest: Option<&ProjectManifest>,
    entry_file: &Path,
    cargo_features: &CargoFeatureSelection,
    cargo_policy: &CargoPolicy,
    package_features: &FeatureSelection,
    sdk_profile_override: Option<&str>,
) -> CliResult<()> {
    validate_oven_lock_policy_impl(
        OvenLockValidationRequest {
            project_root,
            manifest,
            entry_file,
            cargo_features,
            cargo_policy,
            package_features,
            sdk_profile_override,
        },
        None,
    )
}

/// Validate strict lock policy using the compilation session already owned by the invoking command.
pub fn validate_oven_lock_policy_with_session(
    request: OvenLockValidationRequest<'_>,
    session: &CompilationSession,
) -> CliResult<()> {
    validate_oven_lock_policy_impl(request, Some(session))
}

/// Validate strict Oven lock policy using an optional command-owned compilation session.
fn validate_oven_lock_policy_impl(
    request: OvenLockValidationRequest<'_>,
    command_session: Option<&CompilationSession>,
) -> CliResult<()> {
    let OvenLockValidationRequest {
        project_root,
        manifest,
        entry_file,
        cargo_features,
        cargo_policy,
        package_features,
        sdk_profile_override,
    } = request;
    if !cargo_policy.locked && !cargo_policy.frozen {
        return Ok(());
    }
    let Some(manifest) = manifest else {
        return Ok(());
    };

    if let Some(workspace) =
        WorkspaceGraph::discover(manifest.project_root()).map_err(|error| CliError::failure(error.to_string()))?
    {
        let context = collect_workspace_lock_context(
            &workspace,
            manifest.project_root(),
            Some(entry_file),
            cargo_features,
            sdk_profile_override,
            command_session,
        )?;
        let mut resolved = context.resolved;
        merge_project_requirement_dependencies(&mut resolved, &context.project_requirements)?;
        if let Some(message) = strict_git_source_error(&resolved) {
            return Err(CliError::failure(message));
        }
        let fingerprint = compute_resolved_fingerprint_with_sdk_paths(
            &resolved.dependencies,
            &resolved.dev_dependencies,
            cargo_features,
            Some(workspace.root()),
            &context.semantic,
            &semantic_sdk_path_dependencies(&context.project_requirements),
        );
        return validate_oven_existing_lock(
            &workspace.root().join(LOCK_FILENAME),
            &fingerprint,
            "workspace oven.lock is missing; run `incan lock` from any workspace member or the workspace root",
            "workspace oven.lock",
        );
    }

    let context = collect_project_lock_context(
        manifest,
        Some(entry_file),
        cargo_features,
        package_features,
        sdk_profile_override,
        None,
        command_session,
    )?
    .ok_or_else(|| CliError::failure("incan lock requires a FILE argument or at least one [project.scripts] entry"))?;
    let mut resolved = context.resolved;
    merge_project_requirement_dependencies(&mut resolved, &context.project_requirements)?;
    if let Some(message) = strict_git_source_error(&resolved) {
        return Err(CliError::failure(message));
    }
    let fingerprint = compute_resolved_fingerprint_with_sdk_paths(
        &resolved.dependencies,
        &resolved.dev_dependencies,
        cargo_features,
        Some(project_root),
        &context.semantic,
        &semantic_sdk_path_dependencies(&context.project_requirements),
    );
    validate_oven_existing_lock(
        &project_root.join(LOCK_FILENAME),
        &fingerprint,
        "oven.lock is missing; run `incan lock`",
        "oven.lock",
    )
}

/// Compare one already-published canonical lock with an Oven-derived fingerprint.
fn validate_oven_existing_lock(
    lock_path: &Path,
    fingerprint: &str,
    missing_message: &str,
    lock_label: &str,
) -> CliResult<()> {
    if !lock_path.exists() {
        return Err(CliError::failure(missing_message));
    }
    let lock = IncanLock::load(lock_path).map_err(|error| CliError::failure(error.to_string()))?;
    if lock.deps_fingerprint == fingerprint {
        return Ok(());
    }
    Err(CliError::failure(format!(
        "{lock_label} is out of date\n\n\
         \x20 expected deps-fingerprint: {fingerprint}\n\
         \x20   actual deps-fingerprint: {}\n\n\
         Run `incan lock` to refresh the canonical lock before using strict Oven execution.",
        lock.deps_fingerprint
    )))
}

/// Resolve the canonical workspace-root Oven lock from every member plus the caller's backend refinements.
fn resolve_workspace_lock_payload(request: WorkspaceLockResolutionRequest<'_>) -> CliResult<LockResolution> {
    let WorkspaceLockResolutionRequest {
        workspace,
        caller_project_name,
        caller_root,
        caller_resolved,
        caller_project_requirements,
        caller_entry_file,
        cargo_features,
        cargo_policy,
        sdk_profile_override,
    } = request;
    let context = collect_workspace_lock_context(
        workspace,
        caller_root,
        caller_entry_file,
        cargo_features,
        sdk_profile_override,
        None,
    )?;
    let mut resolved = context.resolved;
    let requirements = context.project_requirements;
    merge_project_requirement_dependencies(&mut resolved, &requirements)?;
    let semantic_sdk_paths = semantic_sdk_path_dependencies(&requirements);
    let fingerprint = compute_resolved_fingerprint_with_sdk_paths(
        &resolved.dependencies,
        &resolved.dev_dependencies,
        cargo_features,
        Some(workspace.root()),
        &context.semantic,
        &semantic_sdk_paths,
    );
    let strict = cargo_policy.locked || cargo_policy.frozen;
    if strict && let Some(message) = strict_git_source_error(&resolved) {
        return Err(CliError::failure(message));
    }

    let caller_resolved = caller_resolved.clone();

    let lock_path = workspace.root().join(LOCK_FILENAME);
    if lock_path.exists() {
        let lock = IncanLock::load(&lock_path).map_err(|error| CliError::failure(error.to_string()))?;
        if lock.deps_fingerprint != fingerprint {
            if strict {
                return Err(CliError::failure(format!(
                    "workspace oven.lock is out of date\n\n\
                     \x20 expected deps-fingerprint: {fingerprint}\n\
                     \x20   actual deps-fingerprint: {}\n\n\
                     Run `incan lock` from any workspace member or the workspace root to refresh the canonical lock.",
                    lock.deps_fingerprint
                )));
            }
            eprintln!(
                "warning: workspace oven.lock is out of date; continuing without using it as Oven lock authority \
                 or rewriting it. Run `incan lock` to refresh it."
            );
            return Ok(LockResolution {
                cargo_lock_authority: CargoLockAuthority::Stale,
                cargo_package_name: caller_project_name.to_string(),
                resolved: caller_resolved,
                project_requirements: caller_project_requirements.clone(),
            });
        }
        return Ok(LockResolution {
            cargo_lock_authority: CargoLockAuthority::None,
            cargo_package_name: caller_project_name.to_string(),
            resolved: caller_resolved,
            project_requirements: caller_project_requirements.clone(),
        });
    }

    if strict {
        return Err(CliError::failure(
            "workspace oven.lock is missing; run `incan lock` from any workspace member or the workspace root",
        ));
    }

    let publication_lock = oven_model::lock::acquire_publication_lock(&workspace.root().join(LOCK_FILENAME))
        .map_err(|error| CliError::failure(format!("failed to acquire workspace lock publication guard: {error}")))?;
    generate_oven_lockfile(
        workspace.root(),
        &resolved,
        &requirements,
        cargo_features,
        &context.semantic,
        Some(&publication_lock),
    )?;
    Ok(LockResolution {
        cargo_lock_authority: CargoLockAuthority::None,
        cargo_package_name: caller_project_name.to_string(),
        resolved: caller_resolved,
        project_requirements: caller_project_requirements.clone(),
    })
}

/// Publish the canonical project or workspace lock and retain its exact dependency surface for inspection authority.
///
/// Test-only imports and provider requirements are included in the same whole-project walk used by `incan lock`.
/// The explicit baker passes this returned surface forward instead of entering the collector a second time.
pub fn publish_oven_project_lock(
    project_root: &Path,
    entrypoint: &Path,
    package_features: &FeatureSelection,
) -> CliResult<PublishedOvenProjectLock> {
    let manifest = ProjectManifest::discover(project_root)
        .map_err(|error| CliError::failure(error.to_string()))?
        .ok_or_else(|| CliError::failure("explicit Oven project bake requires a loaf.toml project"))?;
    enforce_project_toolchain_constraint(&manifest)?;
    let cargo_features = CargoFeatureSelection::default().normalized();
    let context = match collect_and_publish_project_lock_for_provider_bake(
        &manifest,
        entrypoint,
        &cargo_features,
        package_features,
    )? {
        ProviderBakeLockPublication::Published(context) => context,
        ProviderBakeLockPublication::Deferred {
            member,
            context,
            reason,
        } => {
            eprintln!(
                "note: workspace lock published without member `{member}`, which cannot resolve until its providers \
                 are baked ({reason}); the next bake or `incan lock` that can see the whole workspace completes it"
            );
            context
        }
    };
    Ok(PublishedOvenProjectLock {
        dependency_surface: context.resolved,
    })
}

/// Collect the canonical lock for an explicit provider bake, tolerating siblings that are not bakeable yet.
///
/// RFC 077 makes the root lock a property of the whole workspace, and `incan lock` rightly refuses to publish a
/// partial one. An explicit provider bake is the step that makes a sibling resolvable in the first place: a leaf
/// provider is baked so that the member consuming it can be, and that consumer cannot contribute to the root lock
/// until it is. Requiring the complete root lock before sealing the leaf is therefore a cycle no bake order can break
/// (#1414). Here a member other than the one being baked may fail to resolve: the root lock is still written with
/// every member that did, so each baked member's own entry — the part of the lock that is its build authority — is
/// on disk from its own bake onward, and the whole-graph fingerprint stays stale until the last member resolves. A
/// single-project bake and a failure in the baked member itself keep the strict path.
fn collect_and_publish_project_lock_for_provider_bake(
    manifest: &ProjectManifest,
    entrypoint: &Path,
    cargo_features: &CargoFeatureSelection,
    package_features: &FeatureSelection,
) -> CliResult<ProviderBakeLockPublication> {
    let Some(workspace) =
        WorkspaceGraph::discover(manifest.project_root()).map_err(|error| CliError::failure(error.to_string()))?
    else {
        let context =
            collect_and_publish_project_lock(manifest, Some(entrypoint), cargo_features, package_features, None)?;
        return Ok(ProviderBakeLockPublication::Published(context));
    };
    let lock_path = workspace.root().join(LOCK_FILENAME);
    let publication_lock = oven_model::lock::acquire_publication_lock(&lock_path)
        .map_err(|error| CliError::failure(format!("failed to acquire workspace lock publication guard: {error}")))?;
    let collection = collect_workspace_lock_context_tolerating(
        &workspace,
        manifest.project_root(),
        Some(entrypoint),
        cargo_features,
        None,
        None,
        true,
    )
    .map_err(|failure| failure.error)?;
    generate_oven_lockfile(
        workspace.root(),
        &collection.context.resolved,
        &collection.context.project_requirements,
        cargo_features,
        &collection.context.semantic,
        Some(&publication_lock),
    )?;
    let mut unresolved = collection.unresolved.into_iter();
    match unresolved.next() {
        None => Ok(ProviderBakeLockPublication::Published(collection.context)),
        Some((member, error)) => Ok(ProviderBakeLockPublication::Deferred {
            member,
            context: collection.context,
            reason: error.message,
        }),
    }
}

/// Generate an Oven-native `oven.lock` without constructing a generated Cargo project.
///
/// The semantic provider graph and dependency fingerprint are the normal-command lock authority. A project that
/// needs an actual Cargo resolution is outside this path and must enter the named `legacy_cargo` publisher.
fn generate_oven_lockfile(
    project_root: &Path,
    resolved: &ResolvedDependencies,
    project_requirements: &ProjectRequirements,
    cargo_features: &CargoFeatureSelection,
    semantic: &SemanticLockState,
    publication_lock: Option<&PublicationLock>,
) -> CliResult<IncanLock> {
    let lock_path = project_root.join(LOCK_FILENAME);
    let owned_publication_lock = if publication_lock.is_none() {
        Some(
            oven_model::lock::acquire_publication_lock(&lock_path)
                .map_err(|error| CliError::failure(format!("failed to acquire lock publication guard: {error}")))?,
        )
    } else {
        None
    };
    let publication_lock = publication_lock.or(owned_publication_lock.as_ref());
    let semantic_sdk_paths = semantic_sdk_path_dependencies(project_requirements);
    let fingerprint = compute_resolved_fingerprint_with_sdk_paths(
        &resolved.dependencies,
        &resolved.dev_dependencies,
        cargo_features,
        Some(project_root),
        semantic,
        &semantic_sdk_paths,
    );
    let lock = IncanLock::new_with_semantic(
        incan_core::version::INCAN_VERSION,
        fingerprint,
        cargo_features.clone(),
        semantic.clone(),
        INERT_CARGO_LOCK_PAYLOAD.to_string(),
    );
    let publication_lock = publication_lock
        .ok_or_else(|| CliError::failure("internal error: lock generation lost its publication guard"))?;
    lock.write_while_locked(&lock_path, publication_lock)
        .map_err(|error| CliError::failure(format!("failed to write oven.lock: {error}")))?;
    Ok(lock)
}

/// Check whether any resolved dependency uses a git branch source, which is forbidden in strict (`--locked` /
/// `--frozen`) mode.
fn strict_git_source_error(resolved: &ResolvedDependencies) -> Option<String> {
    for spec in resolved.dependencies.iter().chain(resolved.dev_dependencies.iter()) {
        if let oven_model::manifest::DependencySource::Git { reference, .. } = &spec.source
            && matches!(reference, oven_model::manifest::GitReference::Branch(_))
        {
            return Some(format!(
                "strict mode forbids git branch dependencies (crate `{}`); use tag or rev",
                spec.crate_name
            ));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::test_support::{empty_project_requirements, empty_resolved};

    #[test]
    fn cargo_lock_payload_override_normalizes_the_supplied_workspace_lock() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let lock_path = temp_dir.path().join("Cargo.lock");
        fs::write(&lock_path, "version = 4\r\n")?;

        assert_eq!(
            cargo_lock_payload_override(Some(lock_path))?,
            Some("version = 4\n".to_string())
        );
        Ok(())
    }

    #[test]
    fn oven_lock_generation_publishes_semantic_state_without_a_cargo_projection()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let lock = generate_oven_lockfile(
            temp_dir.path(),
            &empty_resolved(),
            &empty_project_requirements(),
            &CargoFeatureSelection::default(),
            &SemanticLockState::default(),
            None,
        )?;

        assert_eq!(lock.cargo_lock_payload, INERT_CARGO_LOCK_PAYLOAD);
        assert!(temp_dir.path().join("oven.lock").is_file());
        let state_dir = oven_model::lock::compiler_lock_state_dir(temp_dir.path());
        assert!(
            !state_dir.join("Cargo.toml").exists() && !state_dir.join("target").exists(),
            "Oven lock generation may retain its publication guard but must not create generated-Cargo lock state"
        );
        Ok(())
    }

    #[test]
    fn resolver_publishes_a_missing_semantic_lock_without_a_cargo_projection() -> Result<(), Box<dyn std::error::Error>>
    {
        let temp_dir = tempfile::tempdir()?;
        let project_root = temp_dir.path();
        let manifest_path = project_root.join("loaf.toml");
        let entry_path = project_root.join("src/main.incn");
        fs::create_dir_all(entry_path.parent().ok_or("entry path has no parent")?)?;
        fs::write(
            &manifest_path,
            "[project]\nname = \"semantic_lock_demo\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(&entry_path, "def main() -> None:\n  pass\n")?;
        let manifest = ProjectManifest::from_str(&fs::read_to_string(&manifest_path)?, &manifest_path)?;
        let cargo_features = CargoFeatureSelection::default();
        let cargo_policy = CargoPolicy::default();

        let resolution = resolve_lock_context(LockResolutionRequest {
            project_root,
            project_name: "semantic_lock_demo",
            entry_file: Some(&entry_path),
            manifest: Some(&manifest),
            resolved: &empty_resolved(),
            project_requirements: &empty_project_requirements(),
            cargo_features: &cargo_features,
            cargo_policy: &cargo_policy,
            semantic: Some(&SemanticLockState::default()),
            package_features: None,
            sdk_profile_override: None,
        })?;

        assert!(matches!(resolution.cargo_lock_authority, CargoLockAuthority::None));
        assert_eq!(
            IncanLock::load(&project_root.join("oven.lock"))?.cargo_lock_payload,
            INERT_CARGO_LOCK_PAYLOAD
        );
        let state_dir = oven_model::lock::compiler_lock_state_dir(project_root);
        assert!(
            !state_dir.join("Cargo.toml").exists() && !state_dir.join("Cargo.lock").exists(),
            "normal lock resolution must not construct a generated Cargo projection"
        );
        Ok(())
    }
}
