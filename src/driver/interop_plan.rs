//! Resolving one declared Oven interop target against its canonical lock: the package root that owns it and the
//! lock-fresh contract inspection, execution baking and platform adapters all consume, so none of them reconstructs
//! standalone-versus-workspace lock policy on its own.

use std::fs;
use std::path::{Path, PathBuf};

use crate::driver::error::{CliError, CliResult};
use crate::lockfile::{IncanLock, LOCK_FILENAME};
use crate::manifest::ProjectManifest;
use crate::oven_interop::{LockedInteropTarget, locked_oven_interop_targets};
use crate::workspace::WorkspaceGraph;

/// One exact package root and target declaration validated against the canonical Oven interop lock.
///
/// Inspection, execution baking, and future platform adapters consume this one lock-fresh projection rather than
/// each reconstructing workspace-member and standalone lock policy independently.
pub struct LockedInteropPlanTarget {
    /// Package root that owns the locked package-relative header, artifact, and shim inputs.
    pub project_root: PathBuf,
    /// Current lock-fresh target-specific interop contract.
    pub target: LockedInteropTarget,
}

/// Resolve one package-owned target only when its canonical standalone or workspace lock remains current.
pub fn locked_interop_plan_target(path: &Path, target: &str) -> CliResult<LockedInteropPlanTarget> {
    // ---- Discover the selected package and canonical lock owner ----
    let manifest = ProjectManifest::discover(path)
        .map_err(|error| CliError::failure(error.to_string()))?
        .ok_or_else(|| CliError::failure("Oven interop baking requires a loaf.toml manifest"))?;
    let context = interop_plan_lock_context(&manifest)?;

    // ---- Require exact Oven interop lock freshness ----
    if context.locked != context.current {
        return Err(CliError::failure(
            "oven.lock Oven interop requirements are out of date; run `incan lock` before inspecting or baking a deployment plan",
        ));
    }

    // ---- Select one package-owned immutable target contract ----
    let target = context
        .locked
        .iter()
        .find(|candidate| candidate.target == target)
        .cloned()
        .ok_or_else(|| {
            CliError::failure(format!(
                "Oven interop target `{target}` is not declared and locked by this package"
            ))
        })?;
    Ok(LockedInteropPlanTarget {
        project_root: manifest.project_root().to_path_buf(),
        target,
    })
}

/// Oven interop lock facts selected from either a standalone package or one canonical workspace member projection.
struct InteropPlanLockContext {
    current: Vec<LockedInteropTarget>,
    locked: Vec<LockedInteropTarget>,
}

/// Recompute the selected package's Oven interop receipts and load the matching canonical lock projection.
fn interop_plan_lock_context(manifest: &ProjectManifest) -> CliResult<InteropPlanLockContext> {
    // ---- Standalone package ----
    let Some(workspace) =
        WorkspaceGraph::discover(manifest.project_root()).map_err(|error| CliError::failure(error.to_string()))?
    else {
        let current = locked_oven_interop_targets(manifest)
            .map_err(|error| CliError::failure(format!("invalid Oven interop requirements: {error}")))?;
        let lock = load_interop_plan_lock(&manifest.project_root().join(LOCK_FILENAME))?;
        let locked = lock.semantic.oven.map(|oven| oven.interop).unwrap_or_default();
        return Ok(InteropPlanLockContext { current, locked });
    };

    // ---- Selected workspace member ----
    let canonical_member_root = fs::canonicalize(manifest.project_root()).map_err(|error| {
        CliError::failure(format!(
            "failed to canonicalize interop-plan package root {}: {error}",
            manifest.project_root().display()
        ))
    })?;
    let member = workspace.member_for_root(&canonical_member_root).ok_or_else(|| {
        CliError::failure(format!(
            "interop plan inspection path must select a project member of workspace {}",
            workspace.root().display()
        ))
    })?;
    let effective_manifest = workspace
        .effective_member_manifest(member)
        .map_err(|error| CliError::failure(error.to_string()))?;
    let current = locked_oven_interop_targets(&effective_manifest)
        .map_err(|error| CliError::failure(format!("invalid Oven interop requirements: {error}")))?;

    // ---- Canonical workspace lock projection ----
    let lock = load_interop_plan_lock(&workspace.root().join(LOCK_FILENAME))?;
    let member_root = portable_workspace_member_root(workspace.root(), member.root())?;
    let locked = lock
        .semantic
        .workspace_members
        .into_iter()
        .find(|candidate| candidate.member_root == member_root)
        .ok_or_else(|| {
            CliError::failure(format!(
                "oven.lock does not contain the selected workspace member `{}`; run `incan lock`",
                member.name()
            ))
        })?
        .oven
        .map(|oven| oven.interop)
        .unwrap_or_default();
    Ok(InteropPlanLockContext { current, locked })
}

/// Load the canonical lock with an interop-plan-specific diagnostic.
fn load_interop_plan_lock(path: &Path) -> CliResult<IncanLock> {
    IncanLock::load(path).map_err(|error| {
        CliError::failure(format!(
            "interop plan inspection requires a current oven.lock at {}: {error}",
            path.display()
        ))
    })
}

/// Express one canonical member root in the same portable coordinate space as the workspace lock.
fn portable_workspace_member_root(workspace_root: &Path, member_root: &Path) -> CliResult<String> {
    member_root
        .strip_prefix(workspace_root)
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .map_err(|error| {
            CliError::failure(format!(
                "workspace member {} is not contained by canonical workspace root {}: {error}",
                member_root.display(),
                workspace_root.display()
            ))
        })
}
