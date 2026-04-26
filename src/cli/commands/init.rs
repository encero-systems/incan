//! Project scaffolding for `incan init` and `incan new`.
//!
//! This module owns the filesystem side of RFC 015 project creation. It writes explicit starter files only: an
//! `incan.toml`, a `src/main.incn` entry point, a starter test, and lightweight repository metadata. Manifest shape is
//! delegated to [`WritableManifest`] so generated projects use the same schema model that the rest of the compiler
//! parses.

use std::collections::HashMap;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::Path;

use crate::cli::{CliError, CliResult, ExitCode};
use crate::manifest::{MANIFEST_FILENAME, ProjectSection, WritableManifest};

/// Options shared by `incan init` and the `incan new` implementation path.
#[derive(Debug, Clone, Default)]
pub struct InitOptions<'a> {
    /// Explicit project name; when absent, the target directory name is used.
    pub name: Option<&'a str>,
    /// Initial project version. Must be a complete SemVer version.
    pub version: &'a str,
    /// Optional short project description.
    pub description: Option<&'a str>,
    /// Optional author string, usually `Name <email>`.
    pub author: Option<&'a str>,
    /// Optional SPDX license identifier or expression.
    pub license: Option<&'a str>,
    /// Overwrite generated files that already exist.
    pub force: bool,
    /// Use default metadata without prompting, even when stdin is interactive.
    pub yes: bool,
    /// Infer project metadata from existing source files where supported.
    pub detect: bool,
}

/// Options for `incan new [name]`.
///
/// `incan new` is a thin wrapper around [`init_project`]: it collects metadata, chooses a project directory, checks
/// that the directory can be reused, then delegates scaffold generation to the same code path as `incan init`.
#[derive(Debug, Clone)]
pub struct NewOptions<'a> {
    /// Project name to write into `[project].name`.
    pub name: Option<&'a str>,
    /// Directory to create or reuse for the project root.
    pub dir: Option<&'a Path>,
    /// Optional short project description.
    pub description: Option<&'a str>,
    /// Optional author string, usually `Name <email>`.
    pub author: Option<&'a str>,
    /// Optional SPDX license identifier or expression.
    pub license: Option<&'a str>,
    /// Reuse a non-empty directory and overwrite generated files.
    pub force: bool,
    /// Use default metadata without interactive prompts.
    pub yes: bool,
}

#[derive(Debug, Clone)]
struct ProjectMetadata {
    name: String,
    version: String,
    description: Option<String>,
    author: Option<String>,
    license: Option<String>,
}

#[derive(Debug, Clone)]
struct MetadataDefaults<'a> {
    name: String,
    version: &'a str,
    description: Option<&'a str>,
    author: Option<&'a str>,
    license: Option<&'a str>,
}

/// Initialize a new Incan project with a full scaffold.
///
/// Creates the project directory (if needed), `incan.toml`, `src/main.incn`, and `tests/test_main.incn` so that
/// `incan run` and `incan test` work immediately after init. Existing generated files are preserved unless
/// [`InitOptions::force`] is set.
pub fn init_project(path: &Path, options: InitOptions<'_>) -> CliResult<ExitCode> {
    let manifest_path = path.join(MANIFEST_FILENAME);
    if manifest_path.exists() && !options.force {
        return Err(CliError::failure(format!(
            "Manifest already exists at '{}'",
            manifest_path.display()
        )));
    }

    let mut metadata = collect_project_metadata(
        MetadataDefaults {
            name: resolve_project_name(path, options.name),
            version: options.version,
            description: options.description,
            author: options.author,
            license: options.license,
        },
        should_prompt(options.yes),
    )?;

    if options.detect
        && metadata.name == "incan_project"
        && let Some(detected) = detect_project_name_from_source(path)
    {
        metadata.name = detected;
    }

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
            name: Some(metadata.name.clone()),
            version: Some(metadata.version.clone()),
            description: metadata.description.clone(),
            authors: metadata.author.clone().map(|author| vec![author]),
            license: metadata.license.clone(),
            readme: Some("README.md".to_string()),
            scripts: HashMap::from([("main".to_string(), "src/main.incn".to_string())]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let manifest_content = manifest
        .to_toml()
        .map_err(|e| CliError::failure(format!("Failed to serialize manifest: {}", e)))?;

    write_project_file(&manifest_path, &manifest_content, options.force)?;

    // ---- Write src/main.incn ----
    let main_path = src_dir.join("main.incn");
    let main_content = if options.detect && main_path.exists() {
        None
    } else {
        Some(format!(
            r#""""A fresh Incan project."""

def main() -> None:
    println("Hello from {name}!")
"#,
            name = metadata.name
        ))
    };

    if let Some(content) = main_content {
        write_project_file(&main_path, &content, options.force)?;
    }

    // ---- Write tests/test_main.incn ----
    let test_content = r#"from std.testing import assert_true

def test_placeholder() -> None:
    assert_true(True)
"#;

    write_project_file(&tests_dir.join("test_main.incn"), test_content, options.force)?;
    write_project_file(
        &path.join("README.md"),
        &readme_content(&metadata.name, metadata.description.as_deref()),
        options.force,
    )?;
    write_project_file(&path.join(".gitignore"), "target/\n", options.force)?;

    // ---- Print summary ----
    println!("Created project '{}' at {}", metadata.name, path.display());
    println!();
    println!("  src/main.incn          Entry point");
    println!("  tests/test_main.incn   Starter test");
    println!("  incan.toml             Project manifest");
    println!();
    println!("Run it:   incan run");
    println!("Test it:  incan test");

    Ok(ExitCode::SUCCESS)
}

/// Create a new Incan project directory.
///
/// The directory must be absent, empty, or explicitly reusable with [`NewOptions::force`]. The generated project is
/// binary-style today: it has a `main` script in `incan.toml` pointing at `src/main.incn`.
pub fn new_project(options: NewOptions<'_>) -> CliResult<ExitCode> {
    let prompt = should_prompt(options.yes);
    let default_name = default_new_project_name(options.name, options.dir, prompt)?;
    let metadata = collect_project_metadata(
        MetadataDefaults {
            name: default_name,
            version: "0.1.0",
            description: options.description,
            author: options.author,
            license: options.license,
        },
        prompt,
    )?;
    let project_dir = options
        .dir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| Path::new(&metadata.name).to_path_buf());

    if project_dir.exists() && !project_dir.is_dir() {
        return Err(CliError::failure(format!(
            "Project path '{}' exists and is not a directory",
            project_dir.display()
        )));
    }
    if project_dir.exists()
        && !options.force
        && fs::read_dir(&project_dir)
            .map_err(|e| CliError::failure(format!("Failed to inspect '{}': {}", project_dir.display(), e)))?
            .next()
            .is_some()
    {
        return Err(CliError::failure(format!(
            "Project directory '{}' already exists and is not empty (use --force to reuse it)",
            project_dir.display()
        )));
    }

    init_project(
        &project_dir,
        InitOptions {
            name: Some(&metadata.name),
            version: &metadata.version,
            description: metadata.description.as_deref(),
            author: metadata.author.as_deref(),
            license: metadata.license.as_deref(),
            force: options.force,
            yes: true,
            detect: false,
        },
    )
}

/// Write generated scaffold content to one file.
///
/// Existing files are preserved unless `force` is set.
fn write_project_file(path: &Path, content: &str, force: bool) -> CliResult<()> {
    if path.exists() && !force {
        return Ok(());
    }
    fs::write(path, content).map_err(|e| CliError::failure(format!("Failed to write '{}': {}", path.display(), e)))?;
    Ok(())
}

/// Resolve the project name from explicit input or the target directory.
fn resolve_project_name(path: &Path, explicit_name: Option<&str>) -> String {
    let resolved_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let project_name = explicit_name
        .map(str::to_string)
        .or_else(|| resolved_path.file_name().and_then(|n| n.to_str()).map(str::to_string))
        .unwrap_or_else(|| "incan_project".to_string());
    sanitize_project_name(&project_name)
}

/// Choose the initial default project name for `incan new`.
///
/// Non-interactive callers must provide either `name` or `dir`.
fn default_new_project_name(name: Option<&str>, dir: Option<&Path>, prompt: bool) -> CliResult<String> {
    if let Some(name) = name {
        return Ok(sanitize_project_name(name));
    }
    if let Some(dir) = dir
        && let Some(name) = dir.file_name().and_then(|name| name.to_str())
    {
        return Ok(sanitize_project_name(name));
    }
    if prompt {
        return Ok("incan_project".to_string());
    }
    Err(CliError::failure(
        "Error: `incan new` requires NAME or --dir when running non-interactively",
    ))
}

/// Collect and validate project metadata, prompting when interactive mode allows it.
fn collect_project_metadata(defaults: MetadataDefaults<'_>, prompt: bool) -> CliResult<ProjectMetadata> {
    let mut name = defaults.name;
    let mut version = defaults.version.to_string();
    let mut description = normalize_optional(defaults.description);
    let mut author = normalize_optional(defaults.author);
    let mut license = normalize_optional(defaults.license);

    if prompt {
        name = prompt_default("Project name", &name)?;
        version = prompt_default("Version", &version)?;
        description = prompt_optional("Description", description.as_deref())?;
        author = prompt_optional_skip("Author", author.as_deref())?;
        license = prompt_optional("License", license.as_deref())?;
    }

    let name = sanitize_project_name(&name);
    validate_version(&version)?;

    Ok(ProjectMetadata {
        name,
        version,
        description,
        author,
        license,
    })
}

/// Return whether CLI metadata prompts should run.
fn should_prompt(yes: bool) -> bool {
    !yes && io::stdin().is_terminal()
}

/// Detect a project name from existing source layout when `--detect` is enabled.
fn detect_project_name_from_source(path: &Path) -> Option<String> {
    let src_main = path.join("src/main.incn");
    if src_main.exists() {
        return path
            .canonicalize()
            .ok()
            .and_then(|p| p.file_name().and_then(|name| name.to_str()).map(sanitize_project_name));
    }
    None
}

/// Prompt for one required metadata value, falling back to `default` on empty input.
fn prompt_default(label: &str, default: &str) -> CliResult<String> {
    print!("{label} [{default}]: ");
    io::stdout()
        .flush()
        .map_err(|e| CliError::failure(format!("Failed to prompt for project metadata: {e}")))?;
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .map_err(|e| CliError::failure(format!("Failed to read project metadata: {e}")))?;
    let trimmed = line.trim();
    if trimmed.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(trimmed.to_string())
    }
}

/// Prompt for one optional metadata value, treating empty input as `None`.
fn prompt_optional(label: &str, default: Option<&str>) -> CliResult<Option<String>> {
    let fallback = default.unwrap_or("");
    prompt_default(label, fallback).map(|value| normalize_optional(Some(value.as_str())))
}

/// Prompt for one optional metadata value with an explicit skip sentinel.
fn prompt_optional_skip(label: &str, default: Option<&str>) -> CliResult<Option<String>> {
    let prompt_label = format!("{label} (n to skip)");
    let answer = prompt_default(&prompt_label, default.unwrap_or(""))?;
    if answer.eq_ignore_ascii_case("n") {
        Ok(None)
    } else {
        Ok(normalize_optional(Some(answer.as_str())))
    }
}

/// Normalize optional text fields by trimming whitespace and dropping empties.
fn normalize_optional(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Validate that a project version is a complete SemVer value.
fn validate_version(version: &str) -> CliResult<()> {
    semver::Version::parse(version)
        .map(|_| ())
        .map_err(|e| CliError::failure(format!("Invalid project version '{version}': {e}")))
}

/// Render the starter README for a generated project.
fn readme_content(project_name: &str, description: Option<&str>) -> String {
    let description = description.map(|value| format!("\n{value}\n")).unwrap_or_default();
    format!(
        r#"# {project_name}
{description}
Generated by `incan`.

```bash
incan run
incan test
```
"#
    )
}

/// Sanitize a project name to contain only alphanumeric characters, hyphens, and underscores.
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

        init_project(
            &project_dir,
            InitOptions {
                version: "0.1.0",
                yes: true,
                ..Default::default()
            },
        )?;

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

        init_project(
            &project_dir,
            InitOptions {
                name: Some("custom_name"),
                version: "0.1.0",
                yes: true,
                ..Default::default()
            },
        )?;

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

        let result = init_project(
            &project_dir,
            InitOptions {
                version: "0.1.0",
                yes: true,
                ..Default::default()
            },
        );
        assert!(result.is_err(), "Should fail when manifest already exists");
        Ok(())
    }

    #[test]
    fn init_project_creates_expected_files() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_dir = tmp.path().join("scaffold_test");
        fs::create_dir_all(&project_dir)?;

        init_project(
            &project_dir,
            InitOptions {
                version: "0.1.0",
                yes: true,
                ..Default::default()
            },
        )?;

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

    #[test]
    fn new_project_rejects_non_empty_directory_without_force() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        fs::write(tmp.path().join("existing.txt"), "content")?;

        let result = new_project(NewOptions {
            name: Some("demo"),
            dir: Some(tmp.path()),
            description: None,
            author: None,
            license: None,
            force: false,
            yes: true,
        });

        assert!(
            result.is_err(),
            "new should reject non-empty dirs unless --force is used"
        );
        Ok(())
    }

    #[test]
    fn init_project_writes_optional_metadata() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_dir = tmp.path().join("metadata");
        fs::create_dir_all(&project_dir)?;

        init_project(
            &project_dir,
            InitOptions {
                name: Some("metadata"),
                version: "0.1.0",
                description: Some("A metadata-rich project"),
                author: Some("Danny <danny@example.com>"),
                license: Some("MIT"),
                yes: true,
                ..Default::default()
            },
        )?;

        let manifest_content = fs::read_to_string(project_dir.join("incan.toml"))?;
        assert!(manifest_content.contains(r#"description = "A metadata-rich project""#));
        assert!(manifest_content.contains(r#"authors = ["Danny <danny@example.com>"]"#));
        assert!(manifest_content.contains(r#"license = "MIT""#));
        assert!(manifest_content.contains(r#"readme = "README.md""#));

        let readme_content = fs::read_to_string(project_dir.join("README.md"))?;
        assert!(readme_content.contains("A metadata-rich project"));
        assert!(readme_content.contains("incan run"));
        assert!(readme_content.contains("incan test"));
        assert!(!readme_content.contains("incan run src/main.incn"));
        assert!(!readme_content.contains("incan test tests/"));
        Ok(())
    }

    #[test]
    fn new_project_defaults_name_from_dir_when_noninteractive() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_dir = tmp.path().join("dir_named");

        new_project(NewOptions {
            name: None,
            dir: Some(&project_dir),
            description: Some("Created from --dir"),
            author: None,
            license: None,
            force: false,
            yes: true,
        })?;

        let manifest_content = fs::read_to_string(project_dir.join("incan.toml"))?;
        assert!(manifest_content.contains(r#"name = "dir_named""#));
        assert!(manifest_content.contains(r#"description = "Created from --dir""#));
        Ok(())
    }

    #[test]
    fn new_project_requires_name_or_dir_without_interaction() {
        let result = new_project(NewOptions {
            name: None,
            dir: None,
            description: None,
            author: None,
            license: None,
            force: false,
            yes: true,
        });

        assert!(result.is_err(), "non-interactive new should require a name or dir");
    }

    #[test]
    fn interactive_new_without_name_or_dir_uses_prompt_placeholder() -> Result<(), Box<dyn std::error::Error>> {
        let placeholder = default_new_project_name(None, None, true)?;

        assert_eq!(placeholder, "incan_project");
        Ok(())
    }

    #[test]
    fn init_project_force_overwrites_manifest() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_dir = tmp.path().join("forced");
        fs::create_dir_all(&project_dir)?;
        fs::write(
            project_dir.join("incan.toml"),
            "[project]\nname = \"old\"\nversion = \"0.0.1\"\n",
        )?;

        init_project(
            &project_dir,
            InitOptions {
                name: Some("new_name"),
                version: "0.2.0",
                force: true,
                yes: true,
                detect: false,
                ..Default::default()
            },
        )?;

        let manifest_content = fs::read_to_string(project_dir.join("incan.toml"))?;
        assert!(manifest_content.contains(r#"name = "new_name""#));
        assert!(manifest_content.contains(r#"version = "0.2.0""#));
        Ok(())
    }

    #[test]
    fn init_project_force_overwrites_readme_and_gitignore() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_dir = tmp.path().join("forced_metadata");
        fs::create_dir_all(&project_dir)?;
        fs::write(project_dir.join("README.md"), "# old\n")?;
        fs::write(project_dir.join(".gitignore"), "node_modules/\n")?;

        init_project(
            &project_dir,
            InitOptions {
                name: Some("forced_metadata"),
                version: "0.2.0",
                description: Some("Updated scaffold"),
                force: true,
                yes: true,
                detect: false,
                ..Default::default()
            },
        )?;

        let readme_content = fs::read_to_string(project_dir.join("README.md"))?;
        let gitignore_content = fs::read_to_string(project_dir.join(".gitignore"))?;
        assert!(readme_content.contains("Updated scaffold"));
        assert_eq!(gitignore_content, "target/\n");
        assert!(!gitignore_content.contains("node_modules/"));
        Ok(())
    }

    #[test]
    fn init_project_detect_preserves_existing_main_and_uses_detected_directory_name()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_dir = tmp.path().join("detected");
        let src_dir = project_dir.join("src");
        fs::create_dir_all(&src_dir)?;
        fs::write(
            src_dir.join("main.incn"),
            "def main() -> None:\n    println(\"Hello from existing app\")\n",
        )?;

        init_project(
            &project_dir,
            InitOptions {
                version: "0.1.0",
                detect: true,
                yes: true,
                ..Default::default()
            },
        )?;

        let manifest_content = fs::read_to_string(project_dir.join("incan.toml"))?;
        assert!(manifest_content.contains(r#"name = "detected""#));
        let main_content = fs::read_to_string(project_dir.join("src/main.incn"))?;
        assert!(main_content.contains("Hello from existing app"));
        Ok(())
    }
}
