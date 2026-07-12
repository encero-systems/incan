//! RFC 077 machine-readable workspace topology inspection.

use std::path::Path;

use serde::Serialize;

use crate::cli::{CliError, CliResult, ExitCode};
use crate::workspace::{WORKSPACE_INSPECT_SCHEMA_VERSION, WorkspaceGraph};

/// Emit the validated workspace topology and current implicit command scope as JSON.
pub fn inspect_workspace(path: &Path, workspace: bool, members: &[String]) -> CliResult<ExitCode> {
    let graph = WorkspaceGraph::discover(path)
        .map_err(|error| CliError::failure(error.to_string()))?
        .ok_or_else(|| CliError::failure(format!("no workspace found for {}", path.display())))?;
    let selected = graph
        .select_members(path, workspace, members)
        .map_err(|error| CliError::failure(error.to_string()))?;
    let report = WorkspaceInspectionReport {
        schema_version: WORKSPACE_INSPECT_SCHEMA_VERSION,
        root: graph.root(),
        manifest_path: graph.manifest_path(),
        members: graph
            .members()
            .iter()
            .map(|member| WorkspaceMemberReport {
                name: &member.name,
                path: &member.path,
                manifest_path: &member.manifest_path,
                root_member: member.root_member,
                effective_library_dependencies: graph
                    .effective_library_dependencies_for(member)
                    .unwrap_or(&EMPTY_LIBRARY_DEPENDENCIES),
                effective_rust_dependencies: graph
                    .effective_rust_dependencies_for(member)
                    .unwrap_or(&EMPTY_DEPENDENCIES),
                effective_rust_dev_dependencies: graph
                    .effective_rust_dev_dependencies_for(member)
                    .unwrap_or(&EMPTY_DEPENDENCIES),
            })
            .collect(),
        default_members: graph.default_members(),
        exclusions: graph.exclusions(),
        shared: graph.shared(),
        lock: graph.lock(),
        selected_members: selected.into_iter().map(|member| member.name.as_str()).collect(),
        warnings: graph.warnings(),
    };
    let output = serde_json::to_string_pretty(&report)
        .map_err(|error| CliError::failure(format!("failed to serialize workspace inspection: {error}")))?;
    println!("{output}");
    Ok(ExitCode::SUCCESS)
}

#[derive(Serialize)]
struct WorkspaceInspectionReport<'a> {
    schema_version: u32,
    root: &'a Path,
    manifest_path: &'a Path,
    members: Vec<WorkspaceMemberReport<'a>>,
    default_members: &'a [String],
    exclusions: &'a [String],
    shared: &'a crate::workspace::WorkspaceSharedConfiguration,
    lock: &'a crate::workspace::WorkspaceLockState,
    selected_members: Vec<&'a str>,
    warnings: &'a [String],
}

#[derive(Serialize)]
struct WorkspaceMemberReport<'a> {
    name: &'a str,
    path: &'a Path,
    manifest_path: &'a Path,
    root_member: bool,
    effective_library_dependencies: &'a std::collections::BTreeMap<String, crate::manifest::LibraryDependencySpec>,
    effective_rust_dependencies: &'a std::collections::BTreeMap<String, crate::manifest::DependencySpec>,
    effective_rust_dev_dependencies: &'a std::collections::BTreeMap<String, crate::manifest::DependencySpec>,
}

static EMPTY_DEPENDENCIES: std::sync::LazyLock<std::collections::BTreeMap<String, crate::manifest::DependencySpec>> =
    std::sync::LazyLock::new(std::collections::BTreeMap::new);

static EMPTY_LIBRARY_DEPENDENCIES: std::sync::LazyLock<
    std::collections::BTreeMap<String, crate::manifest::LibraryDependencySpec>,
> = std::sync::LazyLock::new(std::collections::BTreeMap::new);

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn inspect_workspace_accepts_a_rooted_workspace() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        fs::write(
            temp.path().join("incan.toml"),
            "[project]\nname = 'root'\n[workspace]\nmembers = ['packages/*']\n",
        )?;
        fs::create_dir_all(temp.path().join("packages/member"))?;
        fs::write(
            temp.path().join("packages/member/incan.toml"),
            "[project]\nname = 'member'\n",
        )?;

        assert_eq!(inspect_workspace(temp.path(), false, &[])?, ExitCode::SUCCESS);
        Ok(())
    }
}
