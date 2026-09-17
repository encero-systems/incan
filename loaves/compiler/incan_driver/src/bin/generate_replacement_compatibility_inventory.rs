//! Generate the replacement compatibility control-plane projections.

use std::path::PathBuf;

/// Regenerate the developer-facing Markdown and machine-readable JSON projections from checked metadata.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let workspace_root = workspace_root()?;
    let markdown =
        workspace_root.join("workspaces/docs-site/docs/contributing/reference/replacement_compatibility_inventory.md");
    let json = workspace_root
        .join("workspaces/docs-site/docs/contributing/reference/replacement_compatibility_inventory.json");
    incan_driver::replacement_compatibility::write_replacement_compatibility_inventory(&markdown, &json)?;
    Ok(())
}

/// Locate the source checkout so the generator writes projections alongside the checked registry it validated.
fn workspace_root() -> Result<PathBuf, String> {
    let current_dir = std::env::current_dir()
        .map_err(|error| format!("failed to resolve the current workspace directory: {error}"))?;
    if let Some(root) = workspace_ancestor(&current_dir) {
        return Ok(root);
    }
    if let Some(root) = std::env::var_os("INCAN_SOURCE_ROOT")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .filter(|root| root.join("Cargo.toml").is_file() && root.join("loaves/stdlib").is_dir())
    {
        return Ok(root);
    }
    if let Some(root) = workspace_ancestor(&PathBuf::from(env!("CARGO_MANIFEST_DIR"))) {
        return Ok(root);
    }
    Err("could not locate an Incan workspace with checked std.features source".to_string())
}

/// Return the nearest Incan workspace above the process directory before considering an ambient source-root override.
fn workspace_ancestor(path: &std::path::Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|candidate| candidate.join("Cargo.toml").is_file() && candidate.join("loaves/stdlib").is_dir())
        .map(std::path::Path::to_path_buf)
}
