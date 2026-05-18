//! CLI orchestration for RFC 015 project lifecycle commands.
//!
//! This module is deliberately thin around the pure policy modules in [`crate::project_lifecycle`]. It owns
//! command-level concerns: project root discovery, manifest reads and writes, terminal output, JSON rendering, and
//! subprocess execution for `incan env run`. SemVer decisions and environment overlay resolution stay in pure modules
//! so they can be tested without a filesystem or process boundary.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::ValueEnum;
use serde_json::{Value as JsonValue, json};
use toml_edit::{DocumentMut, value};

use crate::cli::{CliError, CliResult, ExitCode};
use crate::manifest::{
    INTERNAL_MANIFEST_OVERRIDE_ENV, INTERNAL_PROJECT_ROOT_OVERRIDE_ENV, MANIFEST_FILENAME, ProjectManifest,
    render_dependency_overlay_manifest,
};
use crate::project_lifecycle::env::{EnvConfigError, EnvConfigSet, EnvRunPreview, ResolvedEnv, resolve_cwd};
use crate::project_lifecycle::toolchain::{ToolchainCompatibility, ToolchainConstraintSet};
use crate::project_lifecycle::version::{VersionBump, VersionChange, VersionRequest};

/// CLI spelling of version bumps accepted by `incan version`.
///
/// These values are parsed by clap and converted into the policy-level [`VersionBump`] enum before applying SemVer
/// rules.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionBumpArg {
    /// Increment the major component and reset minor/patch.
    Major,
    /// Increment the minor component and reset patch.
    Minor,
    /// Increment the patch component.
    Patch,
    /// Start or advance the `alpha.N` prerelease channel.
    Alpha,
    /// Start or advance the `beta.N` prerelease channel.
    Beta,
    /// Start or advance the `rc.N` prerelease channel.
    Rc,
    /// Start or advance the `dev.N` prerelease channel.
    Dev,
}

/// Output formats supported by `incan env list` and `incan env show`.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvOutputFormat {
    /// Human-oriented line-based output.
    Text,
    /// Structured JSON output for scripts and CI.
    Json,
}

/// Parsed options for `incan version`.
///
/// The CLI layer accepts either a bump or an explicit version, then maps the request to [`VersionRequest`] after
/// validating that the flags are mutually exclusive.
pub struct VersionCommandOptions {
    /// Optional SemVer bump name such as `patch` or `dev`.
    pub bump: Option<VersionBumpArg>,
    /// Explicit SemVer value passed with `--set`.
    pub set: Option<String>,
    /// Print the planned change without writing `incan.toml`.
    pub dry_run: bool,
    /// Preserve prerelease metadata for release-core bumps.
    pub keep_prerelease: bool,
    /// Explicit project root or manifest path; otherwise discovery starts from cwd.
    pub project: Option<PathBuf>,
}

/// Apply a project-version change to `incan.toml`.
///
/// The command requires a project manifest with `[project].version`. In dry-run mode it prints the old and new versions
/// without writing. Otherwise it edits only the manifest's project version while preserving the rest of the TOML
/// document through `toml_edit`.
pub fn version_project(options: VersionCommandOptions) -> CliResult<ExitCode> {
    let request = match (options.bump, options.set) {
        (Some(_), Some(_)) => {
            return Err(CliError::failure(
                "Error: `incan version` accepts either a bump name or `--set <version>`, not both",
            ));
        }
        (Some(bump), None) => VersionRequest::Bump {
            bump: bump.into(),
            keep_prerelease: options.keep_prerelease,
        },
        (None, Some(version)) => VersionRequest::Set { version },
        (None, None) => {
            return Err(CliError::failure(
                "Error: `incan version` requires a bump name or `--set <version>`",
            ));
        }
    };

    let manifest_context = load_manifest_context(options.project.as_deref())?;
    let mut document = manifest_context.document;
    let old_version = project_version(&document, &manifest_context.manifest_path)?;
    let change = request
        .apply(&old_version)
        .map_err(|error| CliError::failure(error.to_string()))?;

    print_version_change(&change, &manifest_context.manifest_path, options.dry_run);

    if !options.dry_run {
        set_project_version_in_document(
            &mut document,
            &change.new_version.to_string(),
            &manifest_context.manifest_path,
        )?;
        fs::write(&manifest_context.manifest_path, document.to_string()).map_err(|error| {
            CliError::failure(format!(
                "Failed to write '{}': {error}",
                manifest_context.manifest_path.display()
            ))
        })?;
    }

    Ok(ExitCode::SUCCESS)
}

/// List available project environments.
///
/// Names are returned in deterministic order. The ambient `default` environment is always listed, even when it has no
/// explicit `[tool.incan.envs.default]` table.
pub fn env_list(format: EnvOutputFormat, project: Option<&Path>) -> CliResult<ExitCode> {
    let context = load_env_context(project)?;
    let names = context.config.env_names();
    match format {
        EnvOutputFormat::Text => {
            for name in names {
                println!("{name}");
            }
        }
        EnvOutputFormat::Json => {
            print_json(&json!(names))?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Show project environments.
///
/// With `env_name`, prints one resolved environment in a compact summary view.
/// Without `env_name`, prints an overview of available environments.
pub fn env_show(env_name: Option<&str>, format: EnvOutputFormat, project: Option<&Path>) -> CliResult<ExitCode> {
    let context = load_env_context(project)?;
    match env_name {
        Some(env_name) => {
            let resolved = context.config.resolve_env(env_name).map_err(env_config_error_to_cli)?;
            match format {
                EnvOutputFormat::Text => print_resolved_env(&resolved, &context.project_root)?,
                EnvOutputFormat::Json => print_resolved_env_json(&resolved, &context.project_root)?,
            }
        }
        None => match format {
            EnvOutputFormat::Text => print_env_overview(&context)?,
            EnvOutputFormat::Json => print_env_overview_json(&context)?,
        },
    }
    Ok(ExitCode::SUCCESS)
}

/// Run a configured environment script.
///
/// The script is resolved through [`EnvConfigSet::resolve_run_preview`], then either printed for `--dry-run` or
/// executed with the resolved cwd and environment variables. Extra CLI arguments are appended to the configured argv
/// without shell interpolation.
pub fn env_run(
    env_name: &str,
    script: &str,
    dry_run: bool,
    extra_args: &[String],
    project: Option<&Path>,
) -> CliResult<ExitCode> {
    let context = load_env_context(project)?;
    let preview = context
        .config
        .resolve_run_preview(&context.project_root, env_name, script, extra_args)
        .map_err(env_config_error_to_cli)?;

    if dry_run {
        print_run_preview(&preview)?;
        return Ok(ExitCode::SUCCESS);
    }
    crate::cli::commands::common::enforce_toolchain_constraints(&preview.resolved_env.requires_incan)?;
    reject_recursive_env_run(&preview)?;

    let Some((program, args)) = preview.argv.split_first() else {
        return Err(CliError::failure(format!(
            "script `{script}` in environment `{env_name}` has an empty argv"
        )));
    };
    let active_marker = format!("{env_name}:{script}");
    let internal_override =
        write_internal_manifest_override(&context.manifest_content, &context.manifest_path, &preview.resolved_env)?;
    let status = Command::new(program)
        .args(args)
        .current_dir(&preview.cwd)
        .envs(&preview.env_vars)
        .env(INTERNAL_MANIFEST_OVERRIDE_ENV, internal_override.path())
        .env(INTERNAL_PROJECT_ROOT_OVERRIDE_ENV, &context.project_root)
        .env(
            "INCAN_ENV_ACTIVE",
            extend_active_invocation_stack(env::var("INCAN_ENV_ACTIVE").ok().as_deref(), &active_marker),
        )
        .status()
        .map_err(|error| CliError::failure(format!("failed to run `{}`: {error}", preview.argv.join(" "))))?;

    Ok(ExitCode(status.code().unwrap_or(1)))
}

impl From<VersionBumpArg> for VersionBump {
    /// Translate the CLI bump spelling into the policy-level bump enum.
    fn from(value: VersionBumpArg) -> Self {
        match value {
            VersionBumpArg::Major => Self::Major,
            VersionBumpArg::Minor => Self::Minor,
            VersionBumpArg::Patch => Self::Patch,
            VersionBumpArg::Alpha => Self::Alpha,
            VersionBumpArg::Beta => Self::Beta,
            VersionBumpArg::Rc => Self::Rc,
            VersionBumpArg::Dev => Self::Dev,
        }
    }
}

struct ManifestContext {
    manifest_path: PathBuf,
    document: DocumentMut,
}

struct EnvContext {
    project_root: PathBuf,
    manifest_path: PathBuf,
    manifest_content: String,
    config: EnvConfigSet,
}

struct InternalManifestOverride {
    path: PathBuf,
}

impl InternalManifestOverride {
    /// Return the path to the temporary override manifest file.
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for InternalManifestOverride {
    /// Best-effort cleanup of the temporary override manifest file.
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Load the manifest document used by `incan version`.
fn load_manifest_context(project: Option<&Path>) -> CliResult<ManifestContext> {
    let manifest_path = if let Some(project) = project {
        explicit_project_root(project)?.join(MANIFEST_FILENAME)
    } else {
        discover_lifecycle_manifest()?.path().to_path_buf()
    };
    let document = read_manifest_document(&manifest_path)?;
    Ok(ManifestContext {
        manifest_path,
        document,
    })
}

/// Load manifest-backed env configuration plus the raw manifest content.
fn load_env_context(project: Option<&Path>) -> CliResult<EnvContext> {
    let (project_root, manifest_path, manifest_content, manifest) = if let Some(project) = project {
        let project_root = explicit_project_root(project)?;
        let manifest_path = project_root.join(MANIFEST_FILENAME);
        let manifest_content = read_manifest_content(&manifest_path)?;
        let manifest = ProjectManifest::from_str(&manifest_content, &manifest_path)
            .map_err(|error| CliError::failure(error.to_string()))?;
        (project_root, manifest_path, manifest_content, manifest)
    } else {
        let manifest = discover_lifecycle_manifest()?;
        let manifest_path = manifest.path().to_path_buf();
        (
            manifest.project_root().to_path_buf(),
            manifest_path.clone(),
            read_manifest_content(&manifest_path)?,
            manifest,
        )
    };
    let config = EnvConfigSet::from_manifest(&manifest);
    Ok(EnvContext {
        project_root,
        manifest_path,
        manifest_content,
        config,
    })
}

/// Resolve an explicit `--project` argument to a project root directory.
fn explicit_project_root(path: &Path) -> CliResult<PathBuf> {
    if path.join(MANIFEST_FILENAME).is_file() {
        Ok(path.to_path_buf())
    } else if path.is_file() && path.file_name().is_some_and(|name| name == MANIFEST_FILENAME) {
        Ok(path.parent().unwrap_or_else(|| Path::new(".")).to_path_buf())
    } else {
        Err(CliError::failure(format!(
            "`--project {}` must point to a directory containing incan.toml",
            path.display()
        )))
    }
}

/// Discover the lifecycle manifest by walking upward from the current directory.
fn discover_lifecycle_manifest() -> CliResult<ProjectManifest> {
    let cwd = env::current_dir()
        .map_err(|error| CliError::failure(format!("failed to determine current directory: {error}")))?;
    ProjectManifest::discover(&cwd)
        .map_err(|error| CliError::failure(error.to_string()))?
        .ok_or_else(|| {
            CliError::failure("No incan.toml found; run `incan init` or `incan new <name>` to create a project")
        })
}

/// Read one manifest file into memory as UTF-8 text.
fn read_manifest_content(manifest_path: &Path) -> CliResult<String> {
    fs::read_to_string(manifest_path)
        .map_err(|error| CliError::failure(format!("failed to read '{}': {error}", manifest_path.display())))
}

/// Parse one manifest file into an editable TOML document.
fn read_manifest_document(manifest_path: &Path) -> CliResult<DocumentMut> {
    let content = read_manifest_content(manifest_path)?;
    content
        .parse::<DocumentMut>()
        .map_err(|error| CliError::failure(format!("failed to parse '{}': {error}", manifest_path.display())))
}

/// Read `[project].version` from an editable manifest document.
fn project_version(document: &DocumentMut, manifest_path: &Path) -> CliResult<String> {
    document
        .get("project")
        .and_then(|project| project.get("version"))
        .and_then(|version| version.as_str())
        .map(str::to_string)
        .ok_or_else(|| {
            CliError::failure(format!(
                "manifest '{}' must define [project].version for `incan version`",
                manifest_path.display()
            ))
        })
}

/// Update `[project].version` in an editable manifest document.
fn set_project_version_in_document(document: &mut DocumentMut, version: &str, manifest_path: &Path) -> CliResult<()> {
    let Some(project) = document.get_mut("project").and_then(|item| item.as_table_like_mut()) else {
        return Err(CliError::failure(format!(
            "manifest '{}' must define [project] for `incan version`",
            manifest_path.display()
        )));
    };
    project.insert("version", value(version));
    Ok(())
}

/// Print a summary of one version change.
fn print_version_change(change: &VersionChange, manifest_path: &Path, dry_run: bool) {
    if dry_run {
        println!("dry-run: true");
    }
    println!("old version: {}", change.old_version);
    println!("new version: {}", change.new_version);
    println!("modified files:");
    println!("  {}", manifest_path.display());
}

/// Convert pure env resolution errors into CLI-facing failures.
fn env_config_error_to_cli(error: EnvConfigError) -> CliError {
    CliError::failure(error.to_string())
}

/// Materialize a temporary manifest file that reflects one resolved env overlay.
fn write_internal_manifest_override(
    manifest_content: &str,
    manifest_path: &Path,
    resolved: &ResolvedEnv,
) -> CliResult<InternalManifestOverride> {
    let rendered = render_dependency_overlay_manifest(
        manifest_content,
        manifest_path,
        &resolved.dependencies,
        &resolved.dev_dependencies,
    )
    .map_err(|error| CliError::failure(error.to_string()))?;
    let path = env::temp_dir().join(format!(
        "incan_env_manifest_override_{}_{}.toml",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ));
    fs::write(&path, rendered)
        .map_err(|error| CliError::failure(format!("failed to write internal env manifest override: {error}")))?;
    Ok(InternalManifestOverride { path })
}

/// Reject recursive `incan env run` chains that would re-enter the same env script.
fn reject_recursive_env_run(preview: &EnvRunPreview) -> CliResult<()> {
    let active_marker = format!("{}:{}", preview.env, preview.script);
    if active_stack_contains(env::var("INCAN_ENV_ACTIVE").ok().as_deref(), &active_marker) {
        return Err(CliError::failure(format!(
            "recursive env invocation detected for `{}` `{}`",
            preview.env, preview.script
        )));
    }

    if preview.argv.len() >= 4
        && preview
            .argv
            .first()
            .is_some_and(|program| program == "incan" || program.ends_with("/incan"))
        && preview.argv.get(1).is_some_and(|arg| arg == "env")
        && preview.argv.get(2).is_some_and(|arg| arg == "run")
        && preview.argv.get(3).is_some_and(|arg| arg == &preview.env)
        && preview.argv.get(4).is_some_and(|arg| arg == &preview.script)
    {
        return Err(CliError::failure(format!(
            "recursive env invocation detected for `{}` `{}`",
            preview.env, preview.script
        )));
    }
    Ok(())
}

/// Parse the active env-run marker stack from its environment-variable form.
fn active_invocation_stack(value: Option<&str>) -> Vec<String> {
    value
        .into_iter()
        .flat_map(|value| value.split('\n'))
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

/// Return whether the active env-run stack already contains one marker.
fn active_stack_contains(value: Option<&str>, marker: &str) -> bool {
    active_invocation_stack(value)
        .iter()
        .any(|active_marker| active_marker == marker)
}

/// Append one marker to the serialized env-run stack.
fn extend_active_invocation_stack(current: Option<&str>, next_marker: &str) -> String {
    let mut stack = active_invocation_stack(current);
    stack.push(next_marker.to_string());
    stack.join("\n")
}

/// Print one resolved environment in the human-oriented text format.
fn print_resolved_env(resolved: &ResolvedEnv, project_root: &Path) -> CliResult<()> {
    let compatibility = toolchain_compatibility(&resolved.requires_incan)?;
    println!("{}", resolved.name);
    println!("  overlay chain: {}", resolved.overlay_chain.join(" -> "));
    println!(
        "  requires-incan: {}",
        compatibility
            .effective_requirement
            .as_deref()
            .unwrap_or("unconstrained")
    );
    println!(
        "  active Incan: {} ({})",
        compatibility.active_version,
        toolchain_status(&compatibility)
    );
    println!("  cwd: {}", display_cwd(project_root, resolved.cwd.as_deref()));
    println!("  env vars: {}", resolved.env_vars.len());
    println!("  scripts: {}", resolved.scripts.len());
    println!("  dependencies: {}", resolved.dependencies.len());
    println!("  dev-dependencies: {}", resolved.dev_dependencies.len());

    print_named_string_section("Environment Variables", &resolved.env_vars, |key, value| {
        format!("{key}={value}")
    });
    print_named_vec_section("Scripts", &resolved.scripts, |name, argv| {
        format!("{name:<16} {}", shell_join(argv))
    });
    print_named_value_section("Dependencies", &resolved.dependencies, |name, value| {
        format!("{name:<16} {}", value_to_display_string(value))
    });
    print_named_value_section("Dev Dependencies", &resolved.dev_dependencies, |name, value| {
        format!("{name:<16} {}", value_to_display_string(value))
    });
    Ok(())
}

/// Print one resolved environment as JSON.
fn print_resolved_env_json(resolved: &ResolvedEnv, project_root: &Path) -> CliResult<()> {
    let compatibility = toolchain_compatibility(&resolved.requires_incan)?;
    print_json(&json!({
        "env": resolved.name,
        "overlay_chain": resolved.overlay_chain,
        "requires_incan": toolchain_json(&compatibility),
        "cwd": resolve_cwd(project_root, resolved.cwd.as_deref()).display().to_string(),
        "env_vars": resolved.env_vars,
        "scripts": resolved.scripts,
        "dependencies": resolved.dependencies,
        "dev_dependencies": resolved.dev_dependencies,
    }))
}

/// Print the dry-run preview for `incan env run`.
fn print_run_preview(preview: &EnvRunPreview) -> CliResult<()> {
    let compatibility = toolchain_compatibility(&preview.resolved_env.requires_incan)?;
    println!("env: {}", preview.env);
    println!(
        "requires-incan: {}",
        compatibility
            .effective_requirement
            .as_deref()
            .unwrap_or("unconstrained")
    );
    println!(
        "active Incan: {} ({})",
        compatibility.active_version,
        toolchain_status(&compatibility)
    );
    println!("cwd: {}", preview.cwd.display());
    println!("env-vars:");
    print_map(&preview.env_vars);
    println!("command: {}", shell_join(&preview.argv));
    Ok(())
}

struct EnvSummary {
    name: String,
    requires_incan: String,
    toolchain_status: String,
    cwd: String,
    env_vars: usize,
    scripts: usize,
    dependencies: usize,
    dev_dependencies: usize,
}

/// Print the human-oriented overview of all configured environments.
fn print_env_overview(context: &EnvContext) -> CliResult<()> {
    let summaries = resolve_env_summaries(context)?;
    print_summary_table(&summaries);
    Ok(())
}

/// Print the overview of all configured environments as JSON.
fn print_env_overview_json(context: &EnvContext) -> CliResult<()> {
    let summaries = resolve_env_summaries(context)?;
    print_json(&json!(
        summaries
            .into_iter()
            .map(|summary| json!({
                "name": summary.name,
                "requires_incan": summary.requires_incan,
                "toolchain_status": summary.toolchain_status,
                "cwd": summary.cwd,
                "env_vars": summary.env_vars,
                "scripts": summary.scripts,
                "dependencies": summary.dependencies,
                "dev_dependencies": summary.dev_dependencies,
            }))
            .collect::<Vec<_>>()
    ))
}

/// Resolve every configured environment into one overview summary.
fn resolve_env_summaries(context: &EnvContext) -> CliResult<Vec<EnvSummary>> {
    context
        .config
        .env_names()
        .into_iter()
        .map(|name| {
            let resolved = context.config.resolve_env(&name).map_err(env_config_error_to_cli)?;
            let compatibility = toolchain_compatibility(&resolved.requires_incan)?;
            Ok(EnvSummary {
                name: resolved.name,
                requires_incan: compatibility
                    .effective_requirement
                    .as_deref()
                    .map(str::to_string)
                    .unwrap_or_else(|| "unconstrained".to_string()),
                toolchain_status: toolchain_status(&compatibility).to_string(),
                cwd: display_cwd(&context.project_root, resolved.cwd.as_deref()),
                env_vars: resolved.env_vars.len(),
                scripts: resolved.scripts.len(),
                dependencies: resolved.dependencies.len(),
                dev_dependencies: resolved.dev_dependencies.len(),
            })
        })
        .collect()
}

/// Print the text table used by `incan env show` without a target name.
fn print_summary_table(summaries: &[EnvSummary]) {
    let name_width = summaries
        .iter()
        .map(|summary| summary.name.len())
        .max()
        .unwrap_or(4)
        .max("Name".len());
    let cwd_width = summaries
        .iter()
        .map(|summary| summary.cwd.len())
        .max()
        .unwrap_or(3)
        .max("Cwd".len());
    let requires_width = summaries
        .iter()
        .map(|summary| summary.requires_incan.len())
        .max()
        .unwrap_or(15)
        .max("Requires Incan".len());
    let toolchain_width = summaries
        .iter()
        .map(|summary| summary.toolchain_status.len())
        .max()
        .unwrap_or(10)
        .max("Toolchain".len());
    let env_vars_width = summaries
        .iter()
        .map(|summary| summary.env_vars.to_string().len())
        .max()
        .unwrap_or(8)
        .max("Env Vars".len());
    let scripts_width = summaries
        .iter()
        .map(|summary| summary.scripts.to_string().len())
        .max()
        .unwrap_or(7)
        .max("Scripts".len());
    let deps_width = summaries
        .iter()
        .map(|summary| summary.dependencies.to_string().len())
        .max()
        .unwrap_or(12)
        .max("Dependencies".len());
    let dev_deps_width = summaries
        .iter()
        .map(|summary| summary.dev_dependencies.to_string().len())
        .max()
        .unwrap_or(16)
        .max("Dev Dependencies".len());

    println!(
        "{:<name_width$}  {:<requires_width$}  {:<toolchain_width$}  {:<cwd_width$}  {:>env_vars_width$}  {:>scripts_width$}  {:>deps_width$}  {:>dev_deps_width$}",
        "Name", "Requires Incan", "Toolchain", "Cwd", "Env Vars", "Scripts", "Dependencies", "Dev Dependencies",
    );
    println!(
        "{:-<name_width$}  {:-<requires_width$}  {:-<toolchain_width$}  {:-<cwd_width$}  {:-<env_vars_width$}  {:-<scripts_width$}  {:-<deps_width$}  {:-<dev_deps_width$}",
        "", "", "", "", "", "", "", "",
    );
    for summary in summaries {
        println!(
            "{:<name_width$}  {:<requires_width$}  {:<toolchain_width$}  {:<cwd_width$}  {:>env_vars_width$}  {:>scripts_width$}  {:>deps_width$}  {:>dev_deps_width$}",
            summary.name,
            summary.requires_incan,
            summary.toolchain_status,
            summary.cwd,
            summary.env_vars,
            summary.scripts,
            summary.dependencies,
            summary.dev_dependencies,
        );
    }
}

/// Compute current-toolchain compatibility for CLI display.
fn toolchain_compatibility(constraints: &ToolchainConstraintSet) -> CliResult<ToolchainCompatibility> {
    constraints
        .compatibility_current()
        .map_err(|error| CliError::failure(error.to_string()))
}

/// Render the compact compatibility status used in text tables.
fn toolchain_status(compatibility: &ToolchainCompatibility) -> &'static str {
    if compatibility.effective_requirement.is_none() {
        "unconstrained"
    } else if compatibility.satisfied {
        "satisfied"
    } else {
        "unsatisfied"
    }
}

/// Render toolchain compatibility as JSON.
fn toolchain_json(compatibility: &ToolchainCompatibility) -> JsonValue {
    json!({
        "active_version": compatibility.active_version,
        "effective": compatibility.effective_requirement.as_deref(),
        "satisfied": compatibility.satisfied,
        "layers": compatibility
            .layers
            .iter()
            .map(|layer| json!({
                "source": layer.source.as_str(),
                "requirement": layer.requirement.as_str(),
            }))
            .collect::<Vec<_>>(),
    })
}

/// Print one named string map section in text output.
fn print_named_string_section<F>(title: &str, map: &BTreeMap<String, String>, render: F)
where
    F: Fn(&str, &str) -> String,
{
    if map.is_empty() {
        return;
    }
    println!();
    println!("{title}");
    for (key, value) in map {
        println!("  {}", render(key, value));
    }
}

/// Print one named argv-map section in text output.
fn print_named_vec_section<F>(title: &str, map: &BTreeMap<String, Vec<String>>, render: F)
where
    F: Fn(&str, &[String]) -> String,
{
    if map.is_empty() {
        return;
    }
    println!();
    println!("{title}");
    for (key, value) in map {
        println!("  {}", render(key, value));
    }
}

/// Print one named TOML-value map section in text output.
fn print_named_value_section<F>(title: &str, map: &BTreeMap<String, toml::Value>, render: F)
where
    F: Fn(&str, &toml::Value) -> String,
{
    if map.is_empty() {
        return;
    }
    println!();
    println!("{title}");
    for (key, value) in map {
        println!("  {}", render(key, value));
    }
}

/// Print a flat string map as `key=value` pairs.
fn print_map(map: &BTreeMap<String, String>) {
    if map.is_empty() {
        println!("  (none)");
    } else {
        for (key, value) in map {
            println!("  {key}={value}");
        }
    }
}

/// Convert one TOML value into a compact text representation for CLI output.
fn value_to_display_string(value: &toml::Value) -> String {
    match value {
        toml::Value::String(value) => value.clone(),
        _ => format!("{value:?}"),
    }
}

/// Print one JSON value with pretty formatting.
fn print_json(value: &serde_json::Value) -> CliResult<()> {
    let output = serde_json::to_string_pretty(value)
        .map_err(|error| CliError::failure(format!("failed to serialize JSON output: {error}")))?;
    println!("{output}");
    Ok(())
}

/// Resolve a displayable cwd relative to the project root.
fn display_cwd(project_root: &Path, cwd: Option<&str>) -> String {
    let path = resolve_cwd(project_root, cwd);
    if path == project_root {
        ".".to_string()
    } else if let Ok(stripped) = path.strip_prefix(project_root) {
        let display = stripped.display().to_string();
        if display.is_empty() { ".".to_string() } else { display }
    } else {
        path.display().to_string()
    }
}

/// Join one argv vector into a shell-like display string.
fn shell_join(argv: &[String]) -> String {
    argv.iter()
        .map(|arg| {
            if arg
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || "-_./:=".contains(ch))
            {
                arg.clone()
            } else {
                format!("{arg:?}")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn project_root_discovery_walks_upward() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        fs::write(
            tmp.path().join(MANIFEST_FILENAME),
            "[project]\nname = \"demo\"\nversion = \"0.1.0\"\n",
        )?;
        let nested = tmp.path().join("src/nested");
        fs::create_dir_all(&nested)?;

        let previous = env::current_dir()?;
        env::set_current_dir(&nested)?;
        let resolved = discover_lifecycle_manifest().map(|manifest| manifest.project_root().to_path_buf());
        env::set_current_dir(previous)?;

        assert_eq!(resolved?.canonicalize()?, tmp.path().canonicalize()?);
        Ok(())
    }

    #[test]
    fn recursive_env_run_is_rejected() {
        let preview = EnvRunPreview {
            env: "unit".to_string(),
            script: "test".to_string(),
            cwd: PathBuf::from("."),
            env_vars: BTreeMap::new(),
            argv: vec![
                "incan".to_string(),
                "env".to_string(),
                "run".to_string(),
                "unit".to_string(),
                "test".to_string(),
            ],
            resolved_env: ResolvedEnv {
                name: "unit".to_string(),
                overlay_chain: vec![],
                requires_incan: ToolchainConstraintSet::new(),
                cwd: None,
                env_vars: BTreeMap::new(),
                scripts: BTreeMap::new(),
                dependencies: BTreeMap::new(),
                dev_dependencies: BTreeMap::new(),
            },
        };

        assert!(reject_recursive_env_run(&preview).is_err());
    }

    #[test]
    fn active_invocation_stack_tracks_nested_env_runs() {
        let stack = extend_active_invocation_stack(Some("default:run"), "unit:test");

        assert_eq!(
            active_invocation_stack(Some(&stack)),
            vec!["default:run".to_string(), "unit:test".to_string()]
        );
        assert!(active_stack_contains(Some(&stack), "unit:test"));
    }

    #[test]
    fn env_run_rejects_unsatisfied_requires_incan_before_spawning() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        fs::write(
            tmp.path().join(MANIFEST_FILENAME),
            r#"
            [project]
            name = "demo"
            version = "0.1.0"

            [tool.incan.envs.release]
            requires-incan = ">999.0.0"

            [tool.incan.envs.release.scripts]
            probe = ["incan", "--version"]
            "#,
        )?;

        let previous = env::current_dir()?;
        env::set_current_dir(tmp.path())?;
        let result = env_run("release", "probe", false, &[], None);
        env::set_current_dir(previous)?;

        let error = match result {
            Ok(exit_code) => return Err(format!("expected requires-incan failure, got {exit_code:?}").into()),
            Err(error) => error,
        };
        assert!(
            error.message.contains("does not satisfy requires-incan"),
            "expected requires-incan failure, got: {}",
            error.message
        );
        assert!(
            error.message.contains("env.release.requires-incan"),
            "expected env layer in diagnostic, got: {}",
            error.message
        );
        Ok(())
    }
}
