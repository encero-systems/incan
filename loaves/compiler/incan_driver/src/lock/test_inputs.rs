//! The imports a project's tests add to its lock: discovered through the same session the runner uses, so the
//! lock and `incan test` agree on which modules a test pulls in.

use std::path::{Path, PathBuf};

use crate::error::{CliError, CliResult};
use crate::lock::TestLockInputs;
use crate::modules::collect_rust_dependency_uses;
use crate::session::CompilationSession;

use oven_model::workspace::WorkspaceGraph;

/// Collect Rust imports, originally declared derive demands and stdlib/provider requirements from project tests.
/// Test files retain test-import context; their imported source modules retain normal dependency context.
pub fn collect_test_lock_inputs(
    project_root: &Path,
    workspace: Option<&WorkspaceGraph>,
    session: &CompilationSession,
) -> CliResult<TestLockInputs> {
    let mut inline_imports = Vec::new();
    let mut project_requirement_modules = Vec::new();
    #[cfg(feature = "cli")]
    let mut discovery_inputs = oven_model::observed_inputs::InputObservations::default();
    #[cfg(feature = "cli")]
    let observations = Some(&mut discovery_inputs);
    #[cfg(not(feature = "cli"))]
    let observations = None;
    let test_files = discover_project_test_files_observed(project_root, workspace, session, observations);

    for file_path in test_files {
        let collected = crate::modules::collect_modules_detailed_with_session_at_path(
            file_path.clone(),
            session,
            vec!["test".to_string()],
        )
        .map_err(|failure| CliError::failure(failure.render_human()))?;
        for module in &collected {
            let test_context = module.file_path == file_path;
            inline_imports.extend(collect_rust_dependency_uses(module, test_context));
            #[cfg(feature = "rust_inspect")]
            inline_imports.extend(crate::modules::collect_declared_rust_derive_dependency_uses(
                module,
                session.manifest.as_ref(),
                test_context,
            ));
        }
        project_requirement_modules.extend(collected);
    }

    Ok(TestLockInputs {
        #[cfg(feature = "cli")]
        discovery_inputs,
        inline_imports,
        project_requirement_modules,
    })
}

/// Discover test files owned by one project, excluding descendant workspace members in rooted workspaces.
#[cfg(test)]
fn discover_project_test_files(
    project_root: &Path,
    workspace: Option<&WorkspaceGraph>,
    session: &CompilationSession,
) -> Vec<PathBuf> {
    discover_project_test_files_observed(project_root, workspace, session, None)
}

/// Preserve the canonical test walk's inputs, including files that may gain inline tests and nested-project boundaries.
fn discover_project_test_files_observed(
    project_root: &Path,
    workspace: Option<&WorkspaceGraph>,
    session: &CompilationSession,
    observations: Option<&mut oven_model::observed_inputs::InputObservations>,
) -> Vec<PathBuf> {
    crate::testing::discover_test_files_with_observations(project_root, session, observations)
        .into_iter()
        .filter(|path| {
            workspace.is_none_or(|graph| {
                graph
                    .member_containing_path(path)
                    .is_some_and(|owner| owner.root() == project_root)
            })
        })
        .collect()
}

