//! Portable Oven interop deployment-plan inspection.
//!
//! `incan inspect interop-plan` projects one canonical locked Oven interop target into the versioned handoff that a
//! future Loaf can carry to Gradle, Xcode, or another platform adapter. It does not build, stage, link, sign, or
//! publish the declared inputs.

use std::path::Path;

use clap::ValueEnum;

use crate::{CliError, CliResult, ExitCode};
pub(crate) use incan_driver::interop_plan::locked_interop_plan_target;
use oven_model::oven_interop::{
    InteropDeploymentAction, InteropDeploymentPlan, InteropDeploymentPlatform, interop_deployment_plan,
};

/// Output format for `incan inspect interop-plan`.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteropPlanInspectionFormat {
    /// Concise target and deployment-action summary for terminal use.
    Text,
    /// Deterministic structured handoff for tools.
    Json,
}

/// Inspect one declared target's canonical locked Oven interop deployment handoff.
pub fn inspect_interop_plan(path: &Path, target: &str, format: InteropPlanInspectionFormat) -> CliResult<ExitCode> {
    let locked = locked_interop_plan_target(path, target)?;
    let plan = interop_deployment_plan(&locked.target).map_err(CliError::failure)?;
    render_interop_plan(&plan, format)
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
fn render_capability_requirement(label: &str, requirement: Option<&oven_model::oven_interop::ToolchainRequirement>) {
    let Some(requirement) = requirement else {
        return;
    };
    if let Some(version) = &requirement.version {
        println!("  {label}: {} ({version})", requirement.capability);
    } else {
        println!("  {label}: {}", requirement.capability);
    }
}
