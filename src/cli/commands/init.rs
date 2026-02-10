//! Project scaffolding for `incan init`.
//!
//! Creates the directory structure, manifest file, entry point, and starter test so that a new Incan project works out
//! of the box with `incan run` and `incan test`.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::cli::{CliError, CliResult, ExitCode};
use crate::manifest::{MANIFEST_FILENAME, ProjectSection, WritableManifest};

/// Initialize a new Incan project with a full scaffold.
///
/// Creates the project directory (if needed), `incan.toml`, `src/main.incn`, and `tests/test_main.incn` so that
/// `incan run` and `incan test` work immediately after init.
pub fn init_project(path: &Path, name: Option<&str>, version: &str) -> CliResult<ExitCode> {
    let manifest_path = path.join(MANIFEST_FILENAME);
    if manifest_path.exists() {
        return Err(CliError::failure(format!(
            "Manifest already exists at '{}'",
            manifest_path.display()
        )));
    }

    // ---- Resolve project name ----
    // Canonicalize so that "." resolves to the actual directory name (e.g. "/Users/.../greeter").
    let resolved_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let project_name = name
        .map(|n| n.to_string())
        .or_else(|| {
            resolved_path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "incan_project".to_string());
    let project_name = sanitize_project_name(&project_name);

    // ---- Create directory structure ----
    let src_dir = path.join("src");
    let tests_dir = path.join("tests");
    for dir in [path, &src_dir, &tests_dir] {
        fs::create_dir_all(dir)
            .map_err(|e| CliError::failure(format!("Failed to create directory '{}': {}", dir.display(), e)))?;
    }

    // ---- Write incan.toml ----
    let manifest = WritableManifest {
        project: Some(ProjectSection {
            name: Some(project_name.clone()),
            version: Some(version.to_string()),
            scripts: HashMap::from([("main".to_string(), "src/main.incn".to_string())]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let manifest_content = manifest
        .to_toml()
        .map_err(|e| CliError::failure(format!("Failed to serialize manifest: {}", e)))?;

    write_if_missing(&manifest_path, &manifest_content)?;

    // ---- Write src/main.incn ----
    let main_content = format!(
        r#""""A fresh Incan project."""

def main() -> None:
    println("Hello from {name}!")
"#,
        name = project_name
    );

    write_if_missing(&src_dir.join("main.incn"), &main_content)?;

    // ---- Write tests/test_main.incn ----
    let test_content = r#"from std.testing import assert_true

def test_placeholder() -> None:
    assert_true(True)
"#;

    write_if_missing(&tests_dir.join("test_main.incn"), test_content)?;

    // ---- Print summary ----
    println!("Created project '{project_name}' at {}", path.display());
    println!();
    println!("  src/main.incn          Entry point");
    println!("  tests/test_main.incn   Starter test");
    println!("  incan.toml             Project manifest");
    println!();
    println!("Run it:   incan run src/main.incn");
    println!("Test it:  incan test tests/");

    Ok(ExitCode::SUCCESS)
}

/// Write `content` to `path`, but only if the file does not already exist.
fn write_if_missing(path: &Path, content: &str) -> CliResult<()> {
    if path.exists() {
        return Ok(());
    }
    fs::write(path, content).map_err(|e| CliError::failure(format!("Failed to write '{}': {}", path.display(), e)))?;
    Ok(())
}

/// Sanitize a project name to contain only alphanumeric characters, hyphens,
/// and underscores.
fn sanitize_project_name(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            out.push(ch);
        } else if ch.is_whitespace() {
            out.push('_');
        }
    }
    if out.is_empty() {
        "incan_project".to_string()
    } else {
        out
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // ---- sanitize_project_name ----

    #[test]
    fn sanitize_strips_special_characters() {
        assert_eq!(sanitize_project_name("my project!"), "my_project");
    }

    #[test]
    fn sanitize_preserves_hyphens_and_underscores() {
        assert_eq!(sanitize_project_name("my-cool_project"), "my-cool_project");
    }

    #[test]
    fn sanitize_empty_string_yields_default() {
        assert_eq!(sanitize_project_name(""), "incan_project");
    }

    #[test]
    fn sanitize_all_special_chars_yields_default() {
        assert_eq!(sanitize_project_name("!!!"), "incan_project");
    }

    // ---- init_project name resolution ----

    #[test]
    fn init_project_uses_directory_name_not_default() -> Result<(), Box<dyn std::error::Error>> {
        // Regression: `incan init .` used to produce project name "incan_project" because `Path::new(".").file_name()`
        // returns `None`. The fix canonicalizes the path first.
        let tmp = tempfile::tempdir()?;
        let project_dir = tmp.path().join("my_greeter");
        fs::create_dir_all(&project_dir)?;

        init_project(&project_dir, None, "0.1.0")?;

        let manifest_content = fs::read_to_string(project_dir.join("incan.toml"))?;
        assert!(
            manifest_content.contains(r#"name = "my_greeter""#),
            "Expected project name 'my_greeter' in manifest, got:\n{}",
            manifest_content,
        );
        Ok(())
    }

    #[test]
    fn init_project_respects_explicit_name() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_dir = tmp.path().join("whatever");
        fs::create_dir_all(&project_dir)?;

        init_project(&project_dir, Some("custom_name"), "0.1.0")?;

        let manifest_content = fs::read_to_string(project_dir.join("incan.toml"))?;
        assert!(
            manifest_content.contains(r#"name = "custom_name""#),
            "Expected explicit name 'custom_name' in manifest, got:\n{}",
            manifest_content,
        );
        Ok(())
    }

    #[test]
    fn init_project_rejects_existing_manifest() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_dir = tmp.path().join("existing");
        fs::create_dir_all(&project_dir)?;
        fs::write(project_dir.join("incan.toml"), "")?;

        let result = init_project(&project_dir, None, "0.1.0");
        assert!(result.is_err(), "Should fail when manifest already exists");
        Ok(())
    }

    #[test]
    fn init_project_creates_expected_files() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_dir = tmp.path().join("scaffold_test");
        fs::create_dir_all(&project_dir)?;

        init_project(&project_dir, None, "0.1.0")?;

        assert!(project_dir.join("incan.toml").exists(), "incan.toml should exist");
        assert!(project_dir.join("src/main.incn").exists(), "src/main.incn should exist");
        assert!(
            project_dir.join("tests/test_main.incn").exists(),
            "tests/test_main.incn should exist"
        );

        let main_content = fs::read_to_string(project_dir.join("src/main.incn"))?;
        assert!(
            main_content.contains("Hello from scaffold_test"),
            "main.incn should reference the project name, got:\n{}",
            main_content,
        );
        Ok(())
    }
}
