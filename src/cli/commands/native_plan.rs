//! Portable target-native deployment plan inspection.
//!
//! `incan inspect native-plan` projects one canonical locked native target into the versioned handoff that a future
//! Loaf can carry to Gradle, Xcode, or another platform adapter. It does not build, stage, link, sign, or publish the
//! declared inputs.

use std::fs;
use std::path::Path;

use clap::ValueEnum;

use crate::cli::{CliError, CliResult, ExitCode};
use crate::lockfile::IncanLock;
use crate::manifest::ProjectManifest;
use crate::native_artifact::{
    LockedNativeTarget, NativeDeploymentAction, NativeDeploymentPlan, NativeDeploymentPlatform, locked_native_targets,
    native_deployment_plan,
};
use crate::workspace::WorkspaceGraph;

/// Output format for `incan inspect native-plan`.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativePlanInspectionFormat {
    /// Concise target and deployment-action summary for terminal use.
    Text,
    /// Deterministic structured handoff for tools.
    Json,
}

/// Inspect one declared target's canonical locked native deployment handoff.
pub fn inspect_native_plan(path: &Path, target: &str, format: NativePlanInspectionFormat) -> CliResult<ExitCode> {
    // ---- Discover the selected package and canonical lock owner ----
    let manifest = ProjectManifest::discover(path)
        .map_err(|error| CliError::failure(error.to_string()))?
        .ok_or_else(|| CliError::failure("native plan inspection requires an incan.toml manifest"))?;
    let context = native_plan_lock_context(&manifest)?;

    // ---- Require exact native-lock freshness ----
    if context.locked != context.current {
        return Err(CliError::failure(
            "incan.lock native inputs are out of date; run `incan lock` before inspecting a deployment plan",
        ));
    }

    // ---- Select and render one exact target ----
    let locked_target = context
        .locked
        .iter()
        .find(|candidate| candidate.target == target)
        .ok_or_else(|| {
            CliError::failure(format!(
                "native target `{target}` is not declared and locked by this package"
            ))
        })?;
    let plan = native_deployment_plan(locked_target).map_err(CliError::failure)?;
    render_native_plan(&plan, format)
}

/// Native lock facts selected from either a standalone package or one canonical workspace member projection.
struct NativePlanLockContext {
    current: Vec<LockedNativeTarget>,
    locked: Vec<LockedNativeTarget>,
}

/// Recompute the selected package's native receipts and load the matching canonical lock projection.
fn native_plan_lock_context(manifest: &ProjectManifest) -> CliResult<NativePlanLockContext> {
    // ---- Standalone package ----
    let Some(workspace) =
        WorkspaceGraph::discover(manifest.project_root()).map_err(|error| CliError::failure(error.to_string()))?
    else {
        let current = locked_native_targets(manifest)
            .map_err(|error| CliError::failure(format!("invalid native inputs: {error}")))?;
        let lock = load_native_plan_lock(&manifest.project_root().join("incan.lock"))?;
        return Ok(NativePlanLockContext {
            current,
            locked: lock.semantic.native,
        });
    };

    // ---- Selected workspace member ----
    let canonical_member_root = fs::canonicalize(manifest.project_root()).map_err(|error| {
        CliError::failure(format!(
            "failed to canonicalize native-plan package root {}: {error}",
            manifest.project_root().display()
        ))
    })?;
    let member = workspace.member_for_root(&canonical_member_root).ok_or_else(|| {
        CliError::failure(format!(
            "native plan inspection path must select a project member of workspace {}",
            workspace.root().display()
        ))
    })?;
    let effective_manifest = workspace
        .effective_member_manifest(member)
        .map_err(|error| CliError::failure(error.to_string()))?;
    let current = locked_native_targets(&effective_manifest)
        .map_err(|error| CliError::failure(format!("invalid native inputs: {error}")))?;

    // ---- Canonical workspace lock projection ----
    let lock = load_native_plan_lock(&workspace.root().join("incan.lock"))?;
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
        .native;
    Ok(NativePlanLockContext { current, locked })
}

/// Load the canonical lock with a native-plan-specific diagnostic.
fn load_native_plan_lock(path: &Path) -> CliResult<IncanLock> {
    IncanLock::load(path).map_err(|error| {
        CliError::failure(format!(
            "native plan inspection requires a current incan.lock at {}: {error}",
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
fn render_native_plan(plan: &NativeDeploymentPlan, format: NativePlanInspectionFormat) -> CliResult<ExitCode> {
    match format {
        NativePlanInspectionFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(plan)
                .map_err(|error| CliError::failure(format!("failed to serialize native deployment plan: {error}")))?
        ),
        NativePlanInspectionFormat::Text => render_native_plan_text(plan),
    }
    Ok(ExitCode::SUCCESS)
}

/// Print a concise target-native summary intended for authors rather than platform-tool ingestion.
fn render_native_plan_text(plan: &NativeDeploymentPlan) {
    // ---- Target identity ----
    println!(
        "Native deployment plan {} (schema {})",
        plan.target, plan.schema_version
    );
    println!("  toolchain: {}", plan.toolchain);
    if let Some(sdk) = &plan.sdk {
        println!("  sdk: {sdk}");
    }
    match &plan.platform {
        Some(NativeDeploymentPlatform::Android { api_level }) => println!("  platform: Android API {api_level}"),
        Some(NativeDeploymentPlatform::Ios { deployment_target }) => {
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
            NativeDeploymentAction::StaticLink { input } => {
                println!("  static {}: {} ({})", artifact.name, input.path, input.digest)
            }
            NativeDeploymentAction::Bundle {
                input,
                runtime_name,
                placement,
                minimum_platform,
            } => println!(
                "  bundle {}: {} as {} at {} (minimum {})",
                artifact.name, input.path, runtime_name, placement, minimum_platform
            ),
            NativeDeploymentAction::System { capability } => {
                println!("  system {}: {}", artifact.name, capability)
            }
        }
        if !artifact.dependencies.is_empty() {
            println!("    dependencies: {}", artifact.dependencies.join(", "));
        }
    }

    // ---- Shim and provenance evidence ----
    for shim in &plan.shims {
        println!("  shim {} ({}) -> {}", shim.name, shim.language.as_str(), shim.output);
    }
    if let Some(provenance) = &plan.provenance {
        println!("  provenance: {provenance}");
    }
}
