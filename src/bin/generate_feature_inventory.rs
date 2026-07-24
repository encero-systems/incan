//! Generate the checked standard-library capability inventory reference.

use std::path::PathBuf;
use std::process::Command;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let workspace_root = workspace_root()?;
    build_workspace_cli(&workspace_root)?;
    let source = workspace_root.join("crates/incan_stdlib/stdlib/capabilities.incn");
    let output = workspace_root.join("workspaces/docs-site/docs/language/reference/feature_inventory.md");
    incan::cli::commands::tools::write_feature_inventory_reference_from_source(&source, &output)?;
    Ok(())
}

/// Build the sibling CLI that compiler-backed inventory generation uses to publish checked SDK provider artifacts.
fn build_workspace_cli(workspace_root: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    if prebuilt_workspace_cli(std::env::var_os("CARGO_BIN_EXE_incan"))?.is_some() {
        return Ok(());
    }

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = Command::new(cargo)
        .current_dir(workspace_root)
        .args(["build", "--locked", "--features", "cli", "--bin", "incan"])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("failed to build the incan CLI required for checked feature inventory generation: {status}").into())
    }
}

/// Locate the source checkout at runtime so an archived generator is not tied to its build machine's workspace path.
fn workspace_root() -> Result<PathBuf, String> {
    if let Some(root) = std::env::var_os("INCAN_SOURCE_ROOT").filter(|path| !path.is_empty()) {
        return Ok(PathBuf::from(root));
    }
    let current_dir = std::env::current_dir()
        .map_err(|error| format!("failed to resolve the current workspace directory: {error}"))?;
    if current_dir.join("Cargo.toml").is_file() && current_dir.join("crates/incan_stdlib/stdlib").is_dir() {
        return Ok(current_dir);
    }
    Ok(PathBuf::from(env!("CARGO_MANIFEST_DIR")))
}

/// Validate an explicitly supplied CLI before allowing inventory generation to skip its ordinary workspace build.
fn prebuilt_workspace_cli(executable: Option<std::ffi::OsString>) -> Result<Option<PathBuf>, String> {
    let Some(executable) = executable.filter(|path| !path.is_empty()) else {
        return Ok(None);
    };
    let executable = PathBuf::from(executable);
    if executable.is_file() {
        Ok(Some(executable))
    } else {
        Err(format!(
            "CARGO_BIN_EXE_incan points to {}, but that prebuilt Incan CLI does not exist",
            executable.display()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::prebuilt_workspace_cli;

    #[test]
    fn prebuilt_workspace_cli_requires_an_existing_file() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let executable = temp_dir.path().join("incan");
        std::fs::write(&executable, "test binary")?;

        assert_eq!(
            prebuilt_workspace_cli(Some(executable.clone().into_os_string()))?,
            Some(executable.clone())
        );
        let missing = temp_dir.path().join("missing-incan");
        let error = prebuilt_workspace_cli(Some(missing.clone().into_os_string()))
            .err()
            .ok_or("expected a missing explicit CLI to fail")?;
        assert!(error.contains(&missing.display().to_string()));
        assert_eq!(prebuilt_workspace_cli(None)?, None);
        Ok(())
    }
}
