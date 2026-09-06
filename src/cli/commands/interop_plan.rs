//! Portable Oven interop deployment-plan inspection.
//!
//! `incan inspect interop-plan` projects one canonical locked Oven interop target into the versioned handoff that a
//! future Loaf can carry to Gradle, Xcode, or another platform adapter. It does not build, stage, link, sign, or
//! publish the declared inputs.

use std::fs;
use std::path::{Path, PathBuf};

use clap::ValueEnum;

use crate::cli::{CliError, CliResult, ExitCode};
use crate::lockfile::{IncanLock, LOCK_FILENAME};
use crate::manifest::ProjectManifest;
use crate::oven_interop::{
    InteropDeploymentAction, InteropDeploymentPlan, InteropDeploymentPlatform, LockedInteropTarget,
    interop_deployment_plan, locked_oven_interop_targets,
};
use crate::workspace::WorkspaceGraph;

/// Output format for `incan inspect interop-plan`.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteropPlanInspectionFormat {
    /// Concise target and deployment-action summary for terminal use.
    Text,
    /// Deterministic structured handoff for tools.
    Json,
}

/// One exact package root and target declaration validated against the canonical Oven interop lock.
///
/// Inspection, execution baking, and future platform adapters consume this one lock-fresh projection rather than
/// each reconstructing workspace-member and standalone lock policy independently.
pub(crate) struct LockedInteropPlanTarget {
    /// Package root that owns the locked package-relative header, artifact, and shim inputs.
    pub(crate) project_root: PathBuf,
    /// Current lock-fresh target-specific interop contract.
    pub(crate) target: LockedInteropTarget,
}

/// Inspect one declared target's canonical locked Oven interop deployment handoff.
pub fn inspect_interop_plan(path: &Path, target: &str, format: InteropPlanInspectionFormat) -> CliResult<ExitCode> {
    let locked = locked_interop_plan_target(path, target)?;
    let plan = interop_deployment_plan(&locked.target).map_err(CliError::failure)?;
    render_interop_plan(&plan, format)
}

/// Resolve one package-owned target only when its canonical standalone or workspace lock remains current.
pub(crate) fn locked_interop_plan_target(path: &Path, target: &str) -> CliResult<LockedInteropPlanTarget> {
    // ---- Discover the selected package and canonical lock owner ----
    let manifest = ProjectManifest::discover(path)
        .map_err(|error| CliError::failure(error.to_string()))?
        .ok_or_else(|| CliError::failure("Oven interop baking requires an incan.toml manifest"))?;
    let context = interop_plan_lock_context(&manifest)?;

    // ---- Require exact Oven interop lock freshness ----
    if context.locked != context.current {
        return Err(CliError::failure(
            "incan.lock Oven interop requirements are out of date; run `incan lock` before inspecting or baking a deployment plan",
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
                "incan.lock does not contain the selected workspace member `{}`; run `incan lock`",
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
            "interop plan inspection requires a current incan.lock at {}: {error}",
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

/// Render one stable deployment plan without changing its structured semantics.
fn render_interop_plan(plan: &InteropDeploymentPlan, format: InteropPlanInspectionFormat) -> CliResult<ExitCode> {
    match format {
        InteropPlanInspectionFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(plan).map_err(|error| CliError::failure(format!(
                "failed to serialize Oven interop deployment plan: {error}"
            )))?
        ),
        InteropPlanInspectionFormat::Text => render_interop_plan_text(plan),
    }
    Ok(ExitCode::SUCCESS)
}

/// Print a concise target-interoperability summary intended for authors rather than platform-tool ingestion.
fn render_interop_plan_text(plan: &InteropDeploymentPlan) {
    // ---- Target identity ----
    println!(
        "Oven interop deployment plan {} (schema {})",
        plan.target, plan.schema_version
    );
    render_capability_requirement("toolchain", plan.toolchain.as_ref());
    render_capability_requirement("sdk", plan.sdk.as_ref());
    match &plan.platform {
        Some(InteropDeploymentPlatform::Android { api_level }) => println!("  platform: Android API {api_level}"),
        Some(InteropDeploymentPlatform::Ios { deployment_target }) => {
            println!("  platform: iOS {deployment_target}+")
        }
        None => {}
    }
    for include_root in &plan.include_roots {
        println!("  include-root: {include_root}");
    }
    for definition in &plan.definitions {
        println!("  definition: {definition}");
    }

    // ---- Deployment actions ----
    for artifact in &plan.artifacts {
        match &artifact.action {
            InteropDeploymentAction::StaticLink { input } => {
                println!("  static {}: {} ({})", artifact.name, input.path, input.digest)
            }
            InteropDeploymentAction::Bundle {
                input,
                runtime_name,
                placement,
                minimum_platform,
            } => println!(
                "  bundle {}: {} as {} at {} (minimum {})",
                artifact.name, input.path, runtime_name, placement, minimum_platform
            ),
            InteropDeploymentAction::System { capability } => {
                println!("  system {}: {}", artifact.name, capability)
            }
        }
        if !artifact.dependencies.is_empty() {
            println!("    dependencies: {}", artifact.dependencies.join(", "));
        }
    }
    for binding in &plan.bindings {
        println!(
            "  binding {}::{}: {}",
            binding.module.join("::"),
            binding.name,
            binding.artifacts.join(", ")
        );
    }

    // ---- Shim input evidence ----
    for shim in &plan.shims {
        println!("  shim {} ({}) -> {}", shim.name, shim.language.as_str(), shim.output);
    }
}

/// Print one requested capability without representing it as an Oven-selected local tool.
fn render_capability_requirement(label: &str, requirement: Option<&crate::oven_interop::ToolchainRequirement>) {
    let Some(requirement) = requirement else {
        return;
    };
    if let Some(version) = &requirement.version {
        println!("  {label}: {} ({version})", requirement.capability);
    } else {
        println!("  {label}: {}", requirement.capability);
    }
}
