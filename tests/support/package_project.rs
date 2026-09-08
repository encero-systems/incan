//! Shared project commands for native and non-linking package acceptance.

use std::error::Error;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use crate::support;

/// Run the repository-built compiler with the test harness's coherent SDK/provider selection.
pub(crate) fn command(project: &Path) -> Command {
    let mut command = Command::new(support::incan_binary());
    command
        .current_dir(project)
        .env("INCAN_NO_BANNER", "1")
        .env_remove("INCAN_INTERNAL_PROJECT_ROOT")
        .env_remove("INCAN_INTERNAL_MANIFEST_OVERRIDE");
    command
}

/// Require a command to succeed while retaining complete diagnostics for a failed producer or consumer boundary.
pub(crate) fn success(output: Output, phase: &str) -> Result<Output, Box<dyn Error>> {
    if !output.status.success() {
        return Err(format!(
            "{phase} failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(output)
}

/// Publish a library or native consumer through the normal explicit Oven bake boundary.
pub(crate) fn bake(project: &Path) -> Result<(), Box<dyn Error>> {
    let mut build = command(project);
    build.args(["oven", "bake", "--project", "."]);
    support::configure_explicit_oven_bake_command(&mut build)?;
    success(build.output()?, "Oven bake")?;
    Ok(())
}

/// Create one minimal project without sharing mutable source or generated output with another test.
pub(crate) fn project(
    root: &Path,
    name: &str,
    source_name: &str,
    source: &str,
    dependencies: &str,
) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(root.join("src"))?;
    fs::write(
        root.join("loaf.toml"),
        format!("[project]\nname = \"{name}\"\nversion = \"1.2.3\"\n{dependencies}"),
    )?;
    fs::write(root.join("src").join(source_name), source)?;
    Ok(())
}
