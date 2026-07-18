//! RFC 077 workspace inspection commands.
//!
//! This command is intentionally a thin projection over [`crate::workspace::WorkspaceGraph`]. It must never
//! rediscover members, make selection decisions, or approximate lock ownership independently from the compiler-owned
//! workspace model.

use std::collections::BTreeSet;
use std::env;
use std::path::Path;

use clap::ValueEnum;
use serde_json::{Value, json};

use crate::cli::{CliError, CliResult, ExitCode};
use crate::lockfile::IncanLock;
use crate::manifest::{DependencySource, DependencySpec};
use crate::workspace::{
    EffectiveLibraryDependency, EffectiveRustDependency, WorkspaceDependencyOrigin, WorkspaceError, WorkspaceGraph,
    WorkspaceMemberDependencies, WorkspaceScopeRequest, WorkspaceWarning,
};

/// Output format for `incan workspace inspect`.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceInspectFormat {
    /// Human-readable workspace summary.
    Text,
    /// Stable machine-readable workspace projection.
    Json,
}

/// Inspect the active workspace and the command scope selected by RFC 077 rules.
pub fn workspace_inspect(
    format: WorkspaceInspectFormat,
    select_workspace: bool,
    members: Vec<String>,
) -> CliResult<ExitCode> {
    let current_dir = env::current_dir()
        .map_err(|error| CliError::failure(format!("failed to determine current directory: {error}")))?;
    let graph = WorkspaceGraph::discover(&current_dir)
        .map_err(|error| CliError::failure(error.to_string()))?
        .ok_or_else(|| CliError::failure("no RFC 077 workspace contains the current project"))?;
    let selection = graph
        .resolve_scope(WorkspaceScopeRequest::new(&current_dir, select_workspace, &members))
        .map_err(|error| CliError::failure(error.to_string()))?;
    let report =
        inspection_report(&graph, &selection, &current_dir).map_err(|error| CliError::failure(error.to_string()))?;

    match format {
        WorkspaceInspectFormat::Text => print_text_report(&report),
        WorkspaceInspectFormat::Json => {
            let rendered = serde_json::to_string_pretty(&report)
                .map_err(|error| CliError::failure(format!("failed to serialize workspace inspection: {error}")))?;
            println!("{rendered}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Build the versioned JSON projection without giving the CLI an independent workspace representation.
fn inspection_report(
    graph: &WorkspaceGraph,
    selection: &crate::workspace::WorkspaceSelection<'_>,
    current_dir: &Path,
) -> Result<Value, WorkspaceError> {
    let root_lock_path = graph.root().join("incan.lock");
    let stale_member_locks = graph
        .members()
        .filter(|member| !member.is_root_member())
        .map(|member| member.root().join("incan.lock"))
        .filter(|path| path.is_file())
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();

    let shared_dependencies = graph.shared_dependencies()?;
    let workspace_manifest = graph.workspace_manifest()?;
    let workspace_section = workspace_manifest
        .workspace()
        .ok_or_else(|| WorkspaceError::NotWorkspace {
            manifest_path: graph.manifest_path().to_path_buf(),
        })?;
    let mut used_shared_libraries = BTreeSet::new();
    let mut used_shared_rust = BTreeSet::new();
    let mut used_shared_rust_dev = BTreeSet::new();
    let members = graph
        .members()
        .map(|member| {
            let dependencies = graph.resolve_member_dependencies(member)?;
            used_shared_libraries.extend(
                dependencies
                    .library_dependencies()
                    .iter()
                    .filter(|(_, dependency)| dependency.origin() == WorkspaceDependencyOrigin::Workspace)
                    .map(|(name, _)| name.clone()),
            );
            used_shared_rust.extend(
                dependencies
                    .rust_dependencies()
                    .iter()
                    .filter(|(_, dependency)| dependency.origin() == WorkspaceDependencyOrigin::Workspace)
                    .map(|(name, _)| name.clone()),
            );
            used_shared_rust_dev.extend(
                dependencies
                    .rust_dev_dependencies()
                    .iter()
                    .filter(|(_, dependency)| dependency.origin() == WorkspaceDependencyOrigin::Workspace)
                    .map(|(name, _)| name.clone()),
            );
            let workspace_environment_extends = graph
                .member_manifest(member)
                .into_iter()
                .flat_map(|manifest| manifest.env_sections())
                .filter_map(|(name, env)| {
                    let inherited = env
                        .extends
                        .into_iter()
                        .filter(|parent| parent.starts_with("workspace:"))
                        .collect::<Vec<_>>();
                    (!inherited.is_empty()).then_some((name, json!(inherited)))
                })
                .collect::<serde_json::Map<_, _>>();
            Ok(json!({
                "name": member.name(),
                "root": member.root().display().to_string(),
                "manifest_path": member.manifest_path().display().to_string(),
                "is_root_member": member.is_root_member(),
                "effective_dependencies": effective_dependencies_json(&dependencies),
                "workspace_environment_extends": workspace_environment_extends,
                "capabilities": [],
            }))
        })
        .collect::<Result<Vec<_>, WorkspaceError>>()?;
    let mut warnings = graph.warnings().map(warning_json).collect::<Vec<_>>();
    warnings.extend(unused_shared_dependency_warnings(
        &shared_dependencies,
        &used_shared_libraries,
        &used_shared_rust,
        &used_shared_rust_dev,
    ));
    let lock_state = match IncanLock::load(&root_lock_path) {
        Ok(lock) => json!({
            "status": "present",
            "fingerprint": lock.deps_fingerprint,
            "cargo_features": lock.cargo_features,
            "semantic": lock.semantic,
        }),
        Err(error) if root_lock_path.exists() => json!({
            "status": "invalid",
            "error": error.to_string(),
        }),
        Err(_) => json!({
            "status": "missing",
        }),
    };

    Ok(json!({
        "schema_version": 1,
        "invocation": {
            "current_dir": current_dir.display().to_string(),
        },
        "workspace": {
            "root": graph.root().display().to_string(),
            "manifest_path": graph.manifest_path().display().to_string(),
        },
        "members": members,
        "default_members": graph.default_members().map(|member| member.name()).collect::<Vec<_>>(),
        "exclusions": graph.exclusions().collect::<Vec<_>>(),
        "selected_scope": {
            "origin": scope_origin_name(selection.origin()),
            "members": selection.members().map(|member| json!({
                "name": member.name(),
                "root": member.root().display().to_string(),
            })).collect::<Vec<_>>(),
        },
        "lock": {
            "canonical_path": root_lock_path.display().to_string(),
            "exists": root_lock_path.is_file(),
            "state": lock_state,
            "stale_member_local_locks": stale_member_locks,
        },
        "shared_dependencies": {
            "libraries": shared_dependencies.library_dependencies.iter().map(|(name, spec)| {
                (name.clone(), json!({
                    "path": spec.path.display().to_string(),
                }))
            }).collect::<serde_json::Map<_, _>>(),
            "rust": shared_dependencies.rust_dependencies.iter().map(|(name, spec)| {
                (name.clone(), dependency_spec_json(spec))
            }).collect::<serde_json::Map<_, _>>(),
            "rust_dev": shared_dependencies.rust_dev_dependencies.iter().map(|(name, spec)| {
                (name.clone(), dependency_spec_json(spec))
            }).collect::<serde_json::Map<_, _>>(),
        },
        "shared_environments": workspace_section.envs,
        "shared_policy": workspace_section.policy,
        "shared_sources": workspace_section.sources,
        "capabilities": {
            "status": "member_local_only",
            "reason": "workspace capability application remains unavailable until scoped mutation planning and policy evaluation are implemented",
        },
        "warnings": warnings,
    }))
}

/// Report shared declarations that are present but not explicitly inherited by any member.
fn unused_shared_dependency_warnings(
    shared: &crate::manifest::WorkspaceSharedDependencies,
    used_libraries: &BTreeSet<String>,
    used_rust: &BTreeSet<String>,
    used_rust_dev: &BTreeSet<String>,
) -> Vec<Value> {
    let unused = |declarations: &BTreeSet<String>, used: &BTreeSet<String>, kind: &str| {
        declarations
            .difference(used)
            .map(|name| {
                json!({
                    "kind": "unused_shared_dependency",
                    "dependency_kind": kind,
                    "name": name,
                })
            })
            .collect::<Vec<_>>()
    };
    let mut warnings = unused(
        &shared.library_dependencies.keys().cloned().collect(),
        used_libraries,
        "library",
    );
    warnings.extend(unused(
        &shared.rust_dependencies.keys().cloned().collect(),
        used_rust,
        "rust",
    ));
    warnings.extend(unused(
        &shared.rust_dev_dependencies.keys().cloned().collect(),
        used_rust_dev,
        "rust_dev",
    ));
    warnings
}

/// Render direct and inherited dependencies with enough provenance for consumers to explain Cargo feature union.
fn effective_dependencies_json(dependencies: &WorkspaceMemberDependencies) -> Value {
    json!({
        "libraries": dependencies.library_dependencies().iter().map(|(name, dependency)| {
            (name.clone(), effective_library_dependency_json(dependency))
        }).collect::<serde_json::Map<_, _>>(),
        "rust": dependencies.rust_dependencies().iter().map(|(name, dependency)| {
            (name.clone(), effective_rust_dependency_json(dependency))
        }).collect::<serde_json::Map<_, _>>(),
        "rust_dev": dependencies.rust_dev_dependencies().iter().map(|(name, dependency)| {
            (name.clone(), effective_rust_dependency_json(dependency))
        }).collect::<serde_json::Map<_, _>>(),
    })
}

/// Render one effective Incan library dependency.
fn effective_library_dependency_json(dependency: &EffectiveLibraryDependency) -> Value {
    json!({
        "origin": dependency_origin_name(dependency.origin()),
        "path": dependency.spec().path.display().to_string(),
    })
}

/// Render one effective Rust dependency and its feature inputs.
fn effective_rust_dependency_json(dependency: &EffectiveRustDependency) -> Value {
    let mut value = dependency_spec_json(dependency.spec());
    let Some(object) = value.as_object_mut() else {
        return value;
    };
    object.insert(
        "origin".to_string(),
        Value::String(dependency_origin_name(dependency.origin()).to_string()),
    );
    object.insert("workspace_features".to_string(), json!(dependency.workspace_features()));
    object.insert("member_features".to_string(), json!(dependency.member_features()));
    value
}

/// Render one canonical Rust dependency specification in the stable inspection projection.
fn dependency_spec_json(spec: &DependencySpec) -> Value {
    json!({
        "crate_name": spec.crate_name,
        "version": spec.version,
        "features": spec.features,
        "default_features": spec.default_features,
        "optional": spec.optional,
        "package": spec.package,
        "source": match &spec.source {
            DependencySource::Registry => json!({"kind": "registry"}),
            DependencySource::Path { path } => json!({
                "kind": "path",
                "path": path.display().to_string(),
            }),
            DependencySource::Git { url, reference } => json!({
                "kind": "git",
                "url": url,
                "reference": format!("{reference:?}"),
            }),
        },
    })
}

/// Stable string representation of declaration provenance.
fn dependency_origin_name(origin: WorkspaceDependencyOrigin) -> &'static str {
    match origin {
        WorkspaceDependencyOrigin::Member => "member",
        WorkspaceDependencyOrigin::Workspace => "workspace",
    }
}

/// Render the compact human view from the same JSON projection returned to tooling.
fn print_text_report(report: &Value) {
    let workspace = &report["workspace"];
    let selection = &report["selected_scope"];
    println!("Workspace: {}", workspace["root"].as_str().unwrap_or("<unknown>"));
    println!(
        "Manifest: {}",
        workspace["manifest_path"].as_str().unwrap_or("<unknown>")
    );
    println!("Selected scope ({})", selection["origin"].as_str().unwrap_or("unknown"));
    for member in selection["members"].as_array().into_iter().flatten() {
        let name = member["name"].as_str().unwrap_or("<unknown>");
        let root = member["root"].as_str().unwrap_or("<unknown>");
        println!("  - {name} ({root})");
    }
    let lock = &report["lock"];
    println!(
        "Canonical lock: {}",
        lock["canonical_path"].as_str().unwrap_or("<unknown>")
    );
    if let Some(members) = lock["state"]["semantic"]["workspace_members"].as_array()
        && !members.is_empty()
    {
        println!("Locked semantic member graphs:");
        for member in members {
            let root = member["member_root"].as_str().unwrap_or("<unknown>");
            let profile = member["sdk"]["profile"].as_str().unwrap_or("none");
            let features = member["packages"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|package| package["active_features"].as_array().into_iter().flatten())
                .filter_map(Value::as_str)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
                .join(", ");
            println!("  - {root}: SDK profile {profile}; features [{}]", features);
        }
    }
    if let Some(stale_locks) = lock["stale_member_local_locks"].as_array()
        && !stale_locks.is_empty()
    {
        println!("Stale member-local locks:");
        for path in stale_locks {
            println!("  - {}", path.as_str().unwrap_or("<unknown>"));
        }
    }
}

/// Render one topology warning in the versioned inspection schema.
fn warning_json(warning: &WorkspaceWarning) -> Value {
    match warning {
        WorkspaceWarning::UnmatchedGlob { pattern } => json!({
            "kind": "unmatched_glob",
            "pattern": pattern,
        }),
    }
}

/// Stable string representation for JSON and text inspection consumers.
fn scope_origin_name(origin: crate::workspace::WorkspaceScopeOrigin) -> &'static str {
    match origin {
        crate::workspace::WorkspaceScopeOrigin::CurrentMember => "current_member",
        crate::workspace::WorkspaceScopeOrigin::DefaultMembers => "default_members",
        crate::workspace::WorkspaceScopeOrigin::ImplicitRootMember => "implicit_root_member",
        crate::workspace::WorkspaceScopeOrigin::Workspace => "workspace",
        crate::workspace::WorkspaceScopeOrigin::ExplicitMembers => "explicit_members",
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{inspection_report, scope_origin_name};
    use crate::workspace::{WorkspaceGraph, WorkspaceScopeOrigin, WorkspaceScopeRequest};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn inspection_projection_reports_scope_and_stale_member_locks() -> TestResult {
        let root = tempfile::tempdir()?;
        fs::write(
            root.path().join("incan.toml"),
            r#"
[project]
name = "root"

[workspace]
members = ["packages/member"]
"#,
        )?;
        fs::create_dir_all(root.path().join("packages/member"))?;
        fs::write(
            root.path().join("packages/member/incan.toml"),
            "[project]\nname = \"member\"\n",
        )?;
        fs::write(root.path().join("packages/member/incan.lock"), "obsolete")?;

        let graph = WorkspaceGraph::load_from_root(root.path())?;
        let selection = graph.resolve_scope(WorkspaceScopeRequest::new(root.path(), false, ["member"]))?;
        let report = inspection_report(&graph, &selection, root.path())?;

        assert_eq!(report["schema_version"], 1);
        assert_eq!(report["selected_scope"]["origin"], "explicit_members");
        assert_eq!(report["selected_scope"]["members"][0]["name"], "member");
        assert_eq!(report["lock"]["exists"], false);
        assert_eq!(
            report["lock"]["stale_member_local_locks"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(scope_origin_name(WorkspaceScopeOrigin::Workspace), "workspace");
        Ok(())
    }
}
