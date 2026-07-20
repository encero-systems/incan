//! Generate the checked standard-library capability inventory reference.

use std::path::PathBuf;
use std::process::Command;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    build_workspace_cli()?;
    let output = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("workspaces/docs-site/docs/language/reference/feature_inventory.md");
    incan::cli::commands::tools::write_feature_inventory_reference(&output)?;
    Ok(())
}

/// Build the sibling CLI that compiler-backed inventory generation uses to publish checked SDK provider artifacts.
fn build_workspace_cli() -> Result<(), Box<dyn std::error::Error>> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = Command::new(cargo)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["build", "--locked", "--features", "cli", "--bin", "incan"])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("failed to build the incan CLI required for checked feature inventory generation: {status}").into())
    }
}
