//! Collecting a project's lock context across a workspace: member discovery, merged dependency surfaces, and the
//! test imports that count as ordinary dependencies.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use incan_lang::lang::stdlib;

use crate::cargo_policy::enforce_project_toolchain_constraint;
use crate::error::{CliError, CliResult};
use crate::lock::test_inputs::collect_test_lock_inputs;
use crate::lock::{ProjectLockContext, TestLockInputs, WorkspaceLockCollection, WorkspaceLockMemberFailure};
#[cfg(any(test, feature = "test_support"))]
use crate::lock::{
    record_project_lock_authority_snapshot_read, record_project_lock_context_collection,
    record_project_lock_provider_plan_projection, record_project_lock_session_discovery,
};
use crate::modules::{
    build_source_map, collect_modules_detailed_with_session, collect_rust_dependency_uses, format_dependency_error,
};
use crate::session::CompilationSession;
use incan_frontend::ParsedModule;
use incan_frontend::ast::{Declaration, ImportKind};
use incan_provider::FeatureSelection;
use incan_provider::dependency_resolver::{ResolvedDependencies, resolve_reachable_dependencies};
use incan_provider::inventory::{extend_requirements_with_provider_plan, provider_used_module_paths};
use incan_provider::lock_semantics::semantic_lock_state;
use incan_provider::requirements::{
    ProjectRequirements, collect_project_requirements, merge_project_requirement_dependencies,
    semantic_sdk_path_dependencies,
};
use oven_model::lock::{CargoFeatureSelection, workspace_semantic_lock_state};
use oven_model::manifest::{DependencySpec, ProjectManifest};
use oven_model::workspace::WorkspaceGraph;

/// Collect every member's effective dependency inputs before lock generation.
///
/// Crucially, this does not accept command scope: RFC 077 makes the root lock a property of the whole graph, so a
/// command started in one member cannot narrow the fingerprint or omit another member's feature activation. For the
/// same reason the command's `--features` selection never reaches the lock: every member, the one the command started
/// in included, is recorded with its declared activation. Flags select what one command builds; a shared root file
/// cannot carry each member's transient selection, and a member's entry must read the same whichever command last
/// published the lock, because that entry is part of the member's sealed build authority. Applying one command's
/// flags across the workspace is how an explicit provider bake failed on a package it was not baking (#1414).
pub fn collect_workspace_lock_context(
    workspace: &WorkspaceGraph,
    command_root: &Path,
    entry_file: Option<&Path>,
    cargo_features: &CargoFeatureSelection,
    sdk_profile_override: Option<&str>,
    command_session: Option<&CompilationSession>,
) -> CliResult<ProjectLockContext> {
    collect_workspace_lock_context_by_member(
        workspace,
        command_root,
        entry_file,
        cargo_features,
        sdk_profile_override,
        command_session,
    )
    .map_err(|failure| failure.error)
}

/// [`collect_workspace_lock_context`] that reports which member stopped the collection.
///
/// The explicit provider bake needs the distinction: a failure in a member other than the one being baked is the
/// ordering case it must tolerate, while a failure in the baked member itself is the bake's own error.
fn collect_workspace_lock_context_by_member(
    workspace: &WorkspaceGraph,
    command_root: &Path,
    entry_file: Option<&Path>,
    cargo_features: &CargoFeatureSelection,
    sdk_profile_override: Option<&str>,
    command_session: Option<&CompilationSession>,
) -> Result<ProjectLockContext, WorkspaceLockMemberFailure> {
    collect_workspace_lock_context_tolerating(
        workspace,
        command_root,
        entry_file,
        cargo_features,
        sdk_profile_override,
        command_session,
        false,
    )
    .map(|collection| collection.context)
}

/// Collect the workspace lock, optionally keeping members that resolve when a sibling cannot.
///
/// In strict mode any member failure ends the collection. In tolerant mode a failure in a member other than the one
/// the command was started in is recorded and that member is left out, so the lock can carry every member that has
/// become resolvable so far; the command's own member still fails the whole collection.
#[allow(clippy::too_many_arguments)]
pub fn collect_workspace_lock_context_tolerating(
    workspace: &WorkspaceGraph,
    command_root: &Path,
    entry_file: Option<&Path>,
    cargo_features: &CargoFeatureSelection,
    sdk_profile_override: Option<&str>,
    command_session: Option<&CompilationSession>,
    tolerate_unresolved_siblings: bool,
) -> Result<WorkspaceLockCollection, WorkspaceLockMemberFailure> {
    let outside = |error: CliError| WorkspaceLockMemberFailure { error };
    let explicit_entry = entry_file
        .map(resolve_explicit_lock_entry)
        .transpose()
        .map_err(outside)?;
    let explicit_entry_owner = explicit_entry
        .as_deref()
        .and_then(|entry| workspace.member_containing_path(entry));
    if let Some(entry) = explicit_entry.as_deref()
        && explicit_entry_owner.is_none()
    {
        return Err(outside(CliError::failure(format!(
            "lock entry {} is not contained by any selected workspace member",
            entry.display()
        ))));
    }
    let mut resolved = ResolvedDependencies {
        dependencies: Vec::new(),
        dev_dependencies: Vec::new(),
    };
    let mut project_requirements = ProjectRequirements::default();
    let mut member_semantics = Vec::new();
    let mut has_context = false;
    let mut unresolved = Vec::new();

    let declared_defaults = FeatureSelection::default();
    for member in workspace.members() {
        let member_is_command_target = project_roots_match(command_root, member.root());
        let in_member = |error: CliError| WorkspaceLockMemberFailure { error };
        let manifest = workspace
            .effective_member_manifest(member)
            .map_err(|error| in_member(CliError::failure(error.to_string())))?;
        enforce_project_toolchain_constraint(&manifest).map_err(in_member)?;
        let member_session = command_session.filter(|session| {
            session
                .manifest
                .as_ref()
                .is_some_and(|session_manifest| project_roots_match(session_manifest.project_root(), member.root()))
        });
        let member_entry = explicit_entry.as_deref().filter(|_| member_is_command_target);
        let member_context = match collect_project_lock_context(
            &manifest,
            member_entry,
            cargo_features,
            &declared_defaults,
            sdk_profile_override,
            Some(workspace),
            member_session,
        ) {
            Ok(Some(context)) => context,
            Ok(None) => continue,
            Err(error) if tolerate_unresolved_siblings && !member_is_command_target => {
                unresolved.push((member.name().to_string(), error));
                continue;
            }
            Err(error) => return Err(in_member(error)),
        };
        has_context = true;
        resolved = merge_workspace_resolved_dependencies(&resolved, &member_context.resolved).map_err(outside)?;
        project_requirements =
            merge_workspace_project_requirements(&project_requirements, &member_context.project_requirements)
                .map_err(outside)?;
        member_semantics.push((member.root().to_path_buf(), member_context.semantic));
    }

    if !has_context {
        return Err(outside(CliError::failure(
            "incan lock requires a FILE argument or at least one [project.scripts] entry across the workspace",
        )));
    }
    let semantic = workspace_semantic_lock_state(workspace.root(), member_semantics)
        .map_err(|error| outside(CliError::failure(error)))?;
    Ok(WorkspaceLockCollection {
        context: ProjectLockContext {
            resolved,
            project_requirements,
            semantic,
        },
        unresolved,
    })
}

/// Canonicalize one optional command-line entry before assigning it to a member.
fn resolve_explicit_lock_entry(entry_file: &Path) -> CliResult<PathBuf> {
    let candidate = if entry_file.is_absolute() {
        entry_file.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| CliError::failure(format!("failed to determine current directory: {error}")))?
            .join(entry_file)
    };
    fs::canonicalize(&candidate)
        .map_err(|error| CliError::failure(format!("failed to resolve lock entry {}: {error}", candidate.display())))
}

/// Merge member dependency sets into Cargo's workspace-wide feature union without allowing identity drift.
fn merge_workspace_resolved_dependencies(
    current: &ResolvedDependencies,
    extra: &ResolvedDependencies,
) -> CliResult<ResolvedDependencies> {
    let mut merged = current.clone();
    for candidate in &extra.dependencies {
        merge_workspace_dependency(&mut merged.dependencies, &mut merged.dev_dependencies, candidate, false)?;
    }
    for candidate in &extra.dev_dependencies {
        merge_workspace_dependency(&mut merged.dependencies, &mut merged.dev_dependencies, candidate, true)?;
    }
    merged
        .dependencies
        .sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
    merged
        .dev_dependencies
        .sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
    Ok(merged)
}

/// Merge one member request; normal dependency use wins over dev-only use while features/defaults become a union.
fn merge_workspace_dependency(
    dependencies: &mut Vec<DependencySpec>,
    dev_dependencies: &mut Vec<DependencySpec>,
    candidate: &DependencySpec,
    dev_only: bool,
) -> CliResult<()> {
    if let Some(existing) = dependencies
        .iter_mut()
        .find(|spec| spec.crate_name == candidate.crate_name)
    {
        merge_workspace_dependency_spec(existing, candidate)?;
        return Ok(());
    }

    if let Some(index) = dev_dependencies
        .iter()
        .position(|spec| spec.crate_name == candidate.crate_name)
    {
        let mut existing = dev_dependencies.remove(index);
        merge_workspace_dependency_spec(&mut existing, candidate)?;
        if dev_only {
            dev_dependencies.push(existing);
        } else {
            dependencies.push(existing);
        }
        return Ok(());
    }

    if dev_only {
        dev_dependencies.push(candidate.clone());
    } else {
        dependencies.push(candidate.clone());
    }
    Ok(())
}

/// Merge only Cargo-unifiable member refinements; source, version, rename, and identity remain exact.
fn merge_workspace_dependency_spec(existing: &mut DependencySpec, candidate: &DependencySpec) -> CliResult<()> {
    if existing.version != candidate.version
        || existing.source != candidate.source
        || existing.package != candidate.package
    {
        return Err(CliError::failure(format!(
            "dependency `{}` has incompatible workspace member identities; align version, source, and package at the workspace root",
            candidate.crate_name
        )));
    }
    existing.features.extend(candidate.features.iter().cloned());
    existing.features.sort();
    existing.features.dedup();
    existing.default_features |= candidate.default_features;
    existing.optional &= candidate.optional;
    Ok(())
}

/// Merge stdlib/provider requirements with the same feature-union rules used for member Rust dependencies.
fn merge_workspace_project_requirements(
    current: &ProjectRequirements,
    extra: &ProjectRequirements,
) -> CliResult<ProjectRequirements> {
    let mut stdlib_facets = current.stdlib_facets.clone();
    stdlib_facets.extend(extra.stdlib_facets.iter().cloned());
    stdlib_facets.sort();
    stdlib_facets.dedup();
    let mut dependencies = current.dependencies.clone();
    for candidate in &extra.dependencies {
        if let Some(existing) = dependencies
            .iter_mut()
            .find(|spec| spec.crate_name == candidate.crate_name)
        {
            merge_workspace_dependency_spec(existing, candidate)?;
        } else {
            dependencies.push(candidate.clone());
        }
    }
    dependencies.sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
    let mut sdk_dependency_rebindings = current.sdk_dependency_rebindings.clone();
    sdk_dependency_rebindings.extend(extra.sdk_dependency_rebindings.iter().cloned());
    sdk_dependency_rebindings.sort_by(|left, right| {
        (
            &left.containing_artifact.crate_root,
            &left.provider_name,
            &left.dependency_key,
            &left.source_crate_root,
            &left.active_crate_root,
        )
            .cmp(&(
                &right.containing_artifact.crate_root,
                &right.provider_name,
                &right.dependency_key,
                &right.source_crate_root,
                &right.active_crate_root,
            ))
    });
    sdk_dependency_rebindings.dedup();
    let mut sdk_path_dependencies = current.sdk_path_dependencies.clone();
    for candidate in &extra.sdk_path_dependencies {
        if let Some(existing) = sdk_path_dependencies
            .iter()
            .find(|dependency| dependency.crate_name == candidate.crate_name)
        {
            if existing != candidate {
                return Err(CliError::failure(format!(
                    "SDK/toolchain path dependency `{}` conflicts between workspace requirement contexts",
                    candidate.crate_name
                )));
            }
        } else {
            sdk_path_dependencies.push(candidate.clone());
        }
    }
    sdk_path_dependencies.sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
    let mut sdk_artifact_projections = current.sdk_artifact_projections.clone();
    sdk_artifact_projections.extend(extra.sdk_artifact_projections.iter().cloned());
    sdk_artifact_projections.sort_by(|left, right| left.artifact.crate_root.cmp(&right.artifact.crate_root));
    sdk_artifact_projections.dedup_by(|left, right| left.artifact.crate_root == right.artifact.crate_root);
    Ok(ProjectRequirements {
        stdlib_facets,
        dependencies,
        sdk_dependency_rebindings,
        sdk_path_dependencies,
        sdk_artifact_projections,
    })
}

/// Include provider imports scoped inside `module tests:` when building the project-wide lock context.
fn lock_provider_used_module_paths(modules: &[ParsedModule]) -> BTreeSet<Vec<String>> {
    let mut used = provider_used_module_paths(modules);
    for module in modules {
        for declaration in &module.ast.declarations {
            let Declaration::TestModule(test_module) = &declaration.node else {
                continue;
            };
            for test_declaration in &test_module.body {
                let Declaration::Import(import) = &test_declaration.node else {
                    continue;
                };
                let path = match &import.kind {
                    ImportKind::Module(path) | ImportKind::From { module: path, .. }
                        if path.parent_levels == 0
                            && !path.is_absolute
                            && path.segments.first().map(String::as_str) == Some(stdlib::STDLIB_ROOT) =>
                    {
                        Some(path.segments.clone())
                    }
                    _ => None,
                };
                used.extend(path);
            }
        }
    }
    used
}

/// Return sorted manifest script and conventional library entry paths plus an optional explicitly requested entry.
///
/// A root library need not expose a runnable script. Canonical project and workspace locks nevertheless cover its
/// `src/lib.incn` graph, so a rooted RFC 077 workspace can publish one complete lock from its root without inventing
/// a command-only entrypoint.
fn project_lock_entry_paths(manifest: &ProjectManifest, explicit_entry_file: Option<&Path>) -> Vec<PathBuf> {
    let mut paths = BTreeSet::new();
    if let Some(project) = &manifest.project {
        for script in project.scripts.values() {
            paths.insert(manifest.project_root().join(script));
        }
    }
    let library_entry = manifest.project_root().join("src/lib.incn");
    if library_entry.is_file() {
        paths.insert(library_entry);
    }
    if let Some(file) = explicit_entry_file {
        paths.insert(file.to_path_buf());
    }
    paths.into_iter().collect()
}

/// Compare project ownership independently of the caller's relative or symlinked spelling.
fn project_roots_match(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

/// Collect the project-wide script and owned test dependency inputs used for lock generation and freshness checks.
///
/// When `workspace` is present, descendant member tests are excluded so every test is resolved against the effective
/// manifest of its deepest owning workspace member. Standalone projects retain unrestricted recursive discovery.
pub fn collect_project_lock_context(
    manifest: &ProjectManifest,
    explicit_entry_file: Option<&Path>,
    cargo_features: &CargoFeatureSelection,
    package_features: &FeatureSelection,
    sdk_profile_override: Option<&str>,
    workspace: Option<&WorkspaceGraph>,
    command_session: Option<&CompilationSession>,
) -> CliResult<Option<ProjectLockContext>> {
    #[cfg(any(test, feature = "test_support"))]
    record_project_lock_context_collection();
    let command_session_manifest = command_session
        .map(|session| {
            session.manifest.as_ref().ok_or_else(|| {
                CliError::failure(format!(
                    "project lock collection has no manifest authority for {}",
                    manifest.project_root().display()
                ))
            })
        })
        .transpose()?;
    if command_session_manifest
        .is_some_and(|session_manifest| !project_roots_match(session_manifest.project_root(), manifest.project_root()))
    {
        return Err(CliError::failure(format!(
            "command-owned compilation session belongs to a different project than {}",
            manifest.project_root().display()
        )));
    }
    let seed_manifest = command_session_manifest.unwrap_or(manifest);
    let seed_entry_paths = project_lock_entry_paths(seed_manifest, explicit_entry_file);
    if seed_entry_paths.is_empty() {
        return Ok(None);
    }

    let session_entry = seed_entry_paths
        .first()
        .ok_or_else(|| CliError::failure("project lock collection lost its source entrypoints"))?;
    let discovered_session = if command_session.is_none() {
        #[cfg(any(test, feature = "test_support"))]
        record_project_lock_session_discovery();
        Some(CompilationSession::discover_for_oven(
            session_entry,
            package_features,
            sdk_profile_override,
        )?)
    } else {
        None
    };
    let session = command_session
        .or(discovered_session.as_ref())
        .ok_or_else(|| CliError::failure("project lock collection lost its compilation session"))?;
    let session_manifest = session.manifest.as_ref().ok_or_else(|| {
        CliError::failure(format!(
            "project lock collection has no manifest authority for {}",
            manifest.project_root().display()
        ))
    })?;
    if command_session.is_none() && !project_roots_match(session_manifest.project_root(), manifest.project_root()) {
        return Err(CliError::failure(format!(
            "command-owned compilation session belongs to a different project than {}",
            manifest.project_root().display()
        )));
    }
    #[cfg(any(test, feature = "test_support"))]
    record_project_lock_authority_snapshot_read();
    let entry_paths = project_lock_entry_paths(session_manifest, explicit_entry_file);
    if entry_paths.is_empty() {
        return Ok(None);
    }
    let mut modules = Vec::new();
    for entry_path in entry_paths {
        let entry_modules = collect_modules_detailed_with_session(entry_path.clone(), session)
            .map_err(|failure| CliError::failure(failure.render_human()))?;
        modules.extend(entry_modules.iter().cloned());
    }

    let TestLockInputs {
        inline_imports: test_inline_imports,
        project_requirement_modules: test_requirement_modules,
    } = collect_test_lock_inputs(
        session_manifest.project_root(),
        workspace,
        Some(&session.library_imported_vocab),
        Some(&session.library_imported_dsl_surfaces),
        Some(&session.library_manifest_index),
        session.provider_plan.as_ref(),
        session,
    )?;

    let mut inline_imports = Vec::new();
    for module in &modules {
        inline_imports.extend(collect_rust_dependency_uses(module, false));
    }
    inline_imports.extend(test_inline_imports);
    let mut project_requirement_modules = modules;
    project_requirement_modules.extend(test_requirement_modules);
    let mut project_requirements =
        collect_project_requirements(&project_requirement_modules, &session.library_manifest_index)?;
    #[cfg(any(test, feature = "test_support"))]
    record_project_lock_provider_plan_projection();
    let provider_plan =
        session.provider_plan_for_used_module_paths(lock_provider_used_module_paths(&project_requirement_modules))?;
    extend_requirements_with_provider_plan(&mut project_requirements, &provider_plan)?;
    let semantic_sdk_paths = semantic_sdk_path_dependencies(&project_requirements);
    let semantic = semantic_lock_state(
        session_manifest.project_root(),
        session_manifest.interop_c(),
        session.sdk_inventory.as_deref(),
        session.sdk_components.as_ref(),
        session.package_feature_plan.as_ref(),
        &provider_plan,
        &semantic_sdk_paths,
    )
    .map_err(CliError::failure)?;

    let mut resolved = resolve_reachable_dependencies(Some(session_manifest), &inline_imports, true, cargo_features)
        .map_err(|errors| {
            let mut msg = String::new();
            let sources = build_source_map(&project_requirement_modules);
            for err in errors {
                msg.push_str(&format_dependency_error(&err, &sources));
            }
            CliError::failure(msg.trim_end())
        })?;
    merge_project_requirement_dependencies(&mut resolved, &project_requirements)?;
    Ok(Some(ProjectLockContext {
        resolved,
        project_requirements,
        semantic,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::test_support::registry_dependency;

    #[test]
    fn workspace_lock_merge_unifies_cargo_features_without_permitting_identity_drift()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut first = registry_dependency("serde");
        first.features = vec!["alloc".to_string()];
        first.default_features = false;
        first.optional = true;
        let mut second = registry_dependency("serde");
        second.features = vec!["derive".to_string()];

        let merged = merge_workspace_resolved_dependencies(
            &ResolvedDependencies {
                dependencies: vec![first],
                dev_dependencies: Vec::new(),
            },
            &ResolvedDependencies {
                dependencies: vec![second],
                dev_dependencies: Vec::new(),
            },
        )?;
        let serde = merged.dependencies.first().ok_or("merged serde dependency missing")?;
        assert_eq!(serde.features, vec!["alloc", "derive"]);
        assert!(serde.default_features);
        assert!(!serde.optional);

        let mut incompatible = registry_dependency("serde");
        incompatible.version = Some("2".to_string());
        let error = merge_workspace_resolved_dependencies(
            &merged,
            &ResolvedDependencies {
                dependencies: vec![incompatible],
                dev_dependencies: Vec::new(),
            },
        )
        .err()
        .ok_or("incompatible workspace dependency should fail")?;
        assert!(error.message.contains("incompatible workspace member identities"));
        Ok(())
    }
}
