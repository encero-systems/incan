//! Shared workspace selection for command entry points.
use crate::{CliError, CliResult};
use oven_model::workspace::{ResolvedWorkspaceScope, WorkspaceGraph, WorkspaceScopeRequest};
use std::env;
use std::path::{Path, PathBuf};

/// Resolve the active RFC 077 scope once for a command, retaining compiler-owned selection identity after graph
/// discovery has finished. Commands without workspace selectors preserve their historical single-project behavior
/// when discovery finds no containing workspace.
pub fn resolve_workspace_command_scope(
    select_workspace: bool,
    member_selectors: &[String],
) -> CliResult<Option<ResolvedWorkspaceScope>> {
    resolve_workspace_command_scope_with_library_build_order(select_workspace, member_selectors, false)
}

/// Resolve a workspace scope for a library build, retaining provider-before-consumer execution order from the
/// compiler-owned workspace graph.
pub fn resolve_workspace_library_build_scope(
    select_workspace: bool,
    member_selectors: &[String],
) -> CliResult<Option<ResolvedWorkspaceScope>> {
    resolve_workspace_command_scope_with_library_build_order(select_workspace, member_selectors, true)
}

/// Resolve the active RFC 077 scope once, optionally applying its library-specific execution order.
fn resolve_workspace_command_scope_with_library_build_order(
    select_workspace: bool,
    member_selectors: &[String],
    library_build_order: bool,
) -> CliResult<Option<ResolvedWorkspaceScope>> {
    let current_dir = env::current_dir()
        .map_err(|error| CliError::failure(format!("failed to determine current directory: {error}")))?;
    let workspace = WorkspaceGraph::discover(&current_dir).map_err(|error| CliError::failure(error.to_string()))?;
    let Some(workspace) = workspace else {
        if select_workspace || !member_selectors.is_empty() {
            return Err(CliError::failure(
                "--workspace and --member require an active RFC 077 workspace",
            ));
        }
        return Ok(None);
    };
    let request = WorkspaceScopeRequest::new(&current_dir, select_workspace, member_selectors);
    let scope = if library_build_order {
        workspace.resolve_library_build_scope(request)
    } else {
        workspace
            .resolve_scope(request)
            .map(|selection| selection.to_owned_scope())
    }
    .map_err(|error| CliError::failure(error.to_string()))?;
    Ok(Some(scope))
}

/// Resolve a formatting path against one selected member without allowing a multi-member command to escape scope.
pub fn workspace_member_relative_path(path: &Path, member_root: &Path) -> CliResult<PathBuf> {
    if path == Path::new(".") {
        return Ok(member_root.to_path_buf());
    }
    if path.is_absolute() {
        if path.starts_with(member_root) {
            return Ok(path.to_path_buf());
        }
        return Err(CliError::failure(format!(
            "format path {} is outside selected workspace member {}",
            path.display(),
            member_root.display()
        )));
    }
    Ok(member_root.join(path))
}
