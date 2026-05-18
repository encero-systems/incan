//! Environment configuration and deterministic overlay resolution for `incan env`.
//!
//! This module intentionally stops at pure configuration handling. CLI parsing, process spawning, recursion guards
//! tied to a live process, and terminal formatting are owned by the orchestration layer that calls these APIs.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use thiserror::Error;
use toml::Value;

use crate::manifest::{EnvSection, ProjectManifest};
use crate::project_lifecycle::toolchain::ToolchainConstraintSet;

/// Display label used for the project-level overlay in resolved chains.
pub const PROJECT_BASE_OVERLAY: &str = "project";
const DEFAULT_ENV_NAME: &str = "default";

/// Parsed and optionally base-seeded environment configuration for a project.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EnvConfigSet {
    base: EnvOverlay,
    envs: BTreeMap<String, EnvSection>,
}

/// A deterministic overlay of fields that can participate in environment resolution.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EnvOverlay {
    /// Effective Incan toolchain constraints contributed by the project and env overlays.
    pub requires_incan: ToolchainConstraintSet,
    /// Working directory for scripts. Relative paths are interpreted against the project root.
    pub cwd: Option<String>,
    /// Environment variables to inject into the script process.
    pub env_vars: BTreeMap<String, String>,
    /// Script commands keyed by script name. Each script is already modeled as argv.
    pub scripts: BTreeMap<String, Vec<String>>,
    /// Rust/runtime dependency overlay entries keyed by crate name.
    pub dependencies: BTreeMap<String, Value>,
    /// Rust/runtime development dependency overlay entries keyed by crate name.
    pub dev_dependencies: BTreeMap<String, Value>,
}

/// Fully resolved environment data suitable for `incan env show`.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedEnv {
    /// Target env that was resolved.
    pub name: String,
    /// Overlay labels in application order, beginning with the project base overlay.
    pub overlay_chain: Vec<String>,
    /// Effective Incan toolchain constraints after project/env overlay resolution.
    pub requires_incan: ToolchainConstraintSet,
    /// Final working directory, if any env overlay defined it.
    pub cwd: Option<String>,
    /// Final environment variable map after deterministic overlay merging.
    pub env_vars: BTreeMap<String, String>,
    /// Final script map after deterministic overlay merging.
    pub scripts: BTreeMap<String, Vec<String>>,
    /// Final Rust/runtime dependency map after deterministic overlay merging.
    pub dependencies: BTreeMap<String, Value>,
    /// Final Rust/runtime development dependency map after deterministic overlay merging.
    pub dev_dependencies: BTreeMap<String, Value>,
}

/// Dry-run/display-oriented command preview for `incan env run`.
#[derive(Debug, Clone, PartialEq)]
pub struct EnvRunPreview {
    /// Env used for command resolution.
    pub env: String,
    /// Script name used for command resolution.
    pub script: String,
    /// Resolved working directory. Relative configured cwd values are joined to the project root.
    pub cwd: PathBuf,
    /// Resolved environment variables to inject.
    pub env_vars: BTreeMap<String, String>,
    /// Final argv, including caller-provided passthrough args.
    pub argv: Vec<String>,
    /// Underlying resolved environment for richer display modes.
    pub resolved_env: ResolvedEnv,
}

/// Configuration and resolution failures for `incan env`.
#[derive(Debug, Clone, Error, PartialEq)]
pub enum EnvConfigError {
    /// The `incan.toml` document could not be decoded into the environment schema.
    #[error("failed to parse incan environment configuration: {message}")]
    Parse { message: String },
    /// A requested or inherited environment name is not configured.
    #[error("environment `{name}` is not configured")]
    MissingEnv { name: String },
    /// The requested script is absent from the resolved environment.
    #[error("script `{script}` is not configured in resolved environment `{env}`")]
    MissingScript { env: String, script: String },
    /// The inheritance graph would include the same env overlay more than once.
    #[error("environment `{name}` is included more than once in overlay chain: {}", chain.join(" -> "))]
    DuplicateInclusion { name: String, chain: Vec<String> },
    /// The inheritance graph contains a cycle.
    #[error("environment inheritance cycle detected: {}", cycle.join(" -> "))]
    Cycle { cycle: Vec<String> },
}

impl EnvConfigSet {
    /// Build an env config set from the canonical manifest model.
    #[must_use]
    pub fn from_manifest(manifest: &ProjectManifest) -> Self {
        Self {
            base: EnvOverlay {
                requires_incan: ToolchainConstraintSet::from_project_manifest(manifest),
                dependencies: manifest.env_base_dependency_overlay(),
                dev_dependencies: manifest.env_base_dev_dependency_overlay(),
                ..EnvOverlay::default()
            },
            envs: manifest.env_sections(),
        }
    }

    /// Attach project-level base data that env overlays should merge on top of.
    #[must_use]
    pub fn with_base_overlay(mut self, base: EnvOverlay) -> Self {
        self.base = base;
        self
    }

    /// Return all available env names in deterministic order.
    ///
    /// `default` is ambient and always available, even when it is not explicitly configured in
    /// `[tool.incan.envs.default]`.
    #[must_use]
    pub fn env_names(&self) -> Vec<String> {
        let mut names = Vec::with_capacity(self.envs.len().saturating_add(1));
        names.push(DEFAULT_ENV_NAME.to_string());
        names.extend(
            self.envs
                .keys()
                .filter(|name| name.as_str() != DEFAULT_ENV_NAME)
                .cloned(),
        );
        names
    }

    /// Resolve one env into deterministic, display-friendly data.
    pub fn resolve_env(&self, name: &str) -> Result<ResolvedEnv, EnvConfigError> {
        let target = if name == DEFAULT_ENV_NAME {
            None
        } else {
            Some(
                self.envs
                    .get(name)
                    .ok_or_else(|| EnvConfigError::MissingEnv { name: name.to_string() })?,
            )
        };

        let mut state = ResolveState::new(self.base.clone());

        if name != DEFAULT_ENV_NAME && !target.is_some_and(|target| target.detached) {
            self.resolve_named_overlay(DEFAULT_ENV_NAME, &mut state)?;
        }

        self.resolve_named_overlay(name, &mut state)?;

        Ok(ResolvedEnv {
            name: name.to_string(),
            overlay_chain: state.chain,
            requires_incan: state.overlay.requires_incan,
            cwd: state.overlay.cwd,
            env_vars: state.overlay.env_vars,
            scripts: state.overlay.scripts,
            dependencies: state.overlay.dependencies,
            dev_dependencies: state.overlay.dev_dependencies,
        })
    }

    /// Resolve one script invocation into a dry-run command preview.
    ///
    /// The returned structure is intentionally process-spawn neutral. The caller can render it as text/JSON or use it
    /// as the input to a later executor without re-running resolution.
    pub fn resolve_run_preview(
        &self,
        project_root: &Path,
        env_name: &str,
        script_name: &str,
        extra_args: &[String],
    ) -> Result<EnvRunPreview, EnvConfigError> {
        let resolved_env = self.resolve_env(env_name)?;
        let script_argv = resolved_env
            .scripts
            .get(script_name)
            .ok_or_else(|| EnvConfigError::MissingScript {
                env: env_name.to_string(),
                script: script_name.to_string(),
            })?;

        let mut argv = script_argv.clone();
        argv.extend(extra_args.iter().cloned());

        Ok(EnvRunPreview {
            env: env_name.to_string(),
            script: script_name.to_string(),
            cwd: resolve_cwd(project_root, resolved_env.cwd.as_deref()),
            env_vars: resolved_env.env_vars.clone(),
            argv,
            resolved_env,
        })
    }

    /// Resolve an env's extends first, then apply the env itself.
    fn resolve_named_overlay(&self, name: &str, state: &mut ResolveState) -> Result<(), EnvConfigError> {
        if let Some(position) = state.stack.iter().position(|active| active == name) {
            let mut cycle = state.stack[position..].to_vec();
            cycle.push(name.to_string());
            return Err(EnvConfigError::Cycle { cycle });
        }

        if state.seen.contains(name) {
            let mut chain = state.chain.clone();
            chain.push(name.to_string());
            return Err(EnvConfigError::DuplicateInclusion {
                name: name.to_string(),
                chain,
            });
        }

        let env = if name == DEFAULT_ENV_NAME {
            self.envs.get(name)
        } else {
            Some(
                self.envs
                    .get(name)
                    .ok_or_else(|| EnvConfigError::MissingEnv { name: name.to_string() })?,
            )
        };

        state.seen.insert(name.to_string());
        state.stack.push(name.to_string());

        if let Some(env) = env {
            for extended in &env.extends {
                self.resolve_named_overlay(extended, state)?;
            }
        }

        state.stack.pop();
        if let Some(env) = env {
            state.overlay.apply_env(name, env);
        }
        state.chain.push(name.to_string());
        Ok(())
    }
}

impl EnvOverlay {
    /// Apply one configured env table on top of this overlay using RFC 015 merge rules.
    fn apply_env(&mut self, name: &str, env: &EnvSection) {
        if let Some(requirement) = &env.requires_incan {
            self.requires_incan
                .push(format!("env.{name}.requires-incan"), requirement.clone());
        }
        if let Some(cwd) = &env.cwd {
            self.cwd = Some(cwd.clone());
        }

        self.env_vars
            .extend(env.env_vars.iter().map(|(key, value)| (key.clone(), value.clone())));
        self.scripts
            .extend(env.scripts.iter().map(|(key, value)| (key.clone(), value.clone())));
        merge_dependency_pairs(
            &mut self.dependencies,
            env.dependencies.iter().map(|(key, value)| (key.as_str(), value)),
        );
        merge_dependency_pairs(
            &mut self.dev_dependencies,
            env.dev_dependencies.iter().map(|(key, value)| (key.as_str(), value)),
        );
    }
}

struct ResolveState {
    overlay: EnvOverlay,
    chain: Vec<String>,
    seen: BTreeSet<String>,
    stack: Vec<String>,
}

impl ResolveState {
    /// Create one fresh resolution state seeded with the project-level base overlay.
    fn new(overlay: EnvOverlay) -> Self {
        Self {
            overlay,
            chain: vec![PROJECT_BASE_OVERLAY.to_string()],
            seen: BTreeSet::new(),
            stack: Vec::new(),
        }
    }
}

/// Resolve a configured cwd against the project root for display and execution.
pub(crate) fn resolve_cwd(project_root: &Path, cwd: Option<&str>) -> PathBuf {
    match cwd {
        Some(value) => {
            let path = PathBuf::from(value);
            if path.is_absolute() {
                path
            } else {
                project_root.join(path)
            }
        }
        None => project_root.to_path_buf(),
    }
}

/// Merge one dependency overlay map into the current base map.
fn merge_dependency_pairs<'a>(
    base: &mut BTreeMap<String, Value>,
    overlay: impl IntoIterator<Item = (&'a str, &'a Value)>,
) {
    for (name, overlay_value) in overlay {
        let merged = match base.get(name) {
            Some(base_value) => merge_dependency_value(base_value, overlay_value),
            None => overlay_value.clone(),
        };
        base.insert(name.to_string(), merged);
    }
}

/// Merge one dependency entry using RFC 015 overlay rules.
fn merge_dependency_value(base: &Value, overlay: &Value) -> Value {
    match (base, overlay) {
        (Value::Table(base_table), Value::Table(overlay_table)) => {
            let mut merged = base_table.clone();
            for (key, value) in overlay_table {
                merged.insert(key.clone(), value.clone());
            }

            if let (Some(base_features), Some(overlay_features)) =
                (base_table.get("features"), overlay_table.get("features"))
                && let Some(features) = merged_feature_array(base_features, overlay_features)
            {
                merged.insert("features".to_string(), features);
            }

            Value::Table(merged)
        }
        _ => overlay.clone(),
    }
}

/// Merge two `features` arrays deterministically, preserving set semantics.
fn merged_feature_array(base: &Value, overlay: &Value) -> Option<Value> {
    let base_features = string_array(base)?;
    let overlay_features = string_array(overlay)?;
    let features = base_features
        .into_iter()
        .chain(overlay_features)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(Value::String)
        .collect();
    Some(Value::Array(features))
}

/// Convert one TOML array value into a vector of strings.
fn string_array(value: &Value) -> Option<Vec<String>> {
    value
        .as_array()?
        .iter()
        .map(|entry| entry.as_str().map(str::to_string))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{EnvConfigError, EnvConfigSet, EnvOverlay, PROJECT_BASE_OVERLAY, ResolvedEnv, merge_dependency_value};
    use crate::manifest::ProjectManifest;
    use std::collections::BTreeMap;
    use std::path::Path;
    use toml::Value;

    fn parse_config(source: &str) -> Result<EnvConfigSet, Box<dyn std::error::Error>> {
        let manifest = ProjectManifest::from_str(source, Path::new("incan.toml"))?;
        Ok(EnvConfigSet::from_manifest(&manifest))
    }

    #[test]
    fn parses_env_names_in_deterministic_order() -> Result<(), Box<dyn std::error::Error>> {
        let config = parse_config(
            r#"
            [project]
            name = "demo"
            version = "0.1.0"

            [tool.incan.envs.unit]

            [tool.incan.envs.default]
            "#,
        )?;

        assert_eq!(config.env_names(), vec!["default".to_string(), "unit".to_string()]);
        Ok(())
    }

    #[test]
    fn ambient_default_is_listed_when_undeclared() -> Result<(), Box<dyn std::error::Error>> {
        let config = parse_config(
            r#"
            [tool.incan.envs.unit]
            "#,
        )?;

        assert_eq!(config.env_names(), vec!["default".to_string(), "unit".to_string()]);
        Ok(())
    }

    #[test]
    fn ambient_default_resolves_without_explicit_configuration() -> Result<(), Box<dyn std::error::Error>> {
        let base = EnvOverlay {
            env_vars: BTreeMap::from([("INCAN_NO_BANNER".to_string(), "1".to_string())]),
            ..EnvOverlay::default()
        };
        let config = parse_config("")?.with_base_overlay(base);

        let resolved = config.resolve_env("default")?;

        assert_eq!(
            resolved.overlay_chain,
            vec![PROJECT_BASE_OVERLAY.to_string(), "default".to_string()]
        );
        assert_eq!(resolved.env_vars.get("INCAN_NO_BANNER").map(String::as_str), Some("1"));
        Ok(())
    }

    #[test]
    fn resolves_default_extends_and_target_in_overlay_order() -> Result<(), Box<dyn std::error::Error>> {
        let config = parse_config(
            r#"
            [tool.incan.envs.default]
            env-vars = { SHARED = "default", DEFAULT_ONLY = "1" }

            [tool.incan.envs.default.scripts]
            test = ["incan", "test"]

            [tool.incan.envs.unit]
            env-vars = { SHARED = "unit", UNIT_ONLY = "1" }

            [tool.incan.envs.unit.scripts]
            test = ["incan", "test", "--unit"]

            [tool.incan.envs.docs]
            extends = ["unit"]
            cwd = "workspaces/docs-site"
            env-vars = { DOCS_ONLY = "1" }

            [tool.incan.envs.docs.scripts]
            docs_build = ["python3", "-m", "mkdocs", "build", "-q"]
            "#,
        )?;

        let resolved = config.resolve_env("docs")?;

        assert_eq!(
            resolved.overlay_chain,
            vec![PROJECT_BASE_OVERLAY, "default", "unit", "docs"]
        );
        assert_eq!(resolved.cwd.as_deref(), Some("workspaces/docs-site"));
        assert_eq!(resolved.env_vars.get("SHARED").map(String::as_str), Some("unit"));
        assert_eq!(resolved.env_vars.get("DEFAULT_ONLY").map(String::as_str), Some("1"));
        assert_eq!(resolved.env_vars.get("UNIT_ONLY").map(String::as_str), Some("1"));
        assert_eq!(resolved.env_vars.get("DOCS_ONLY").map(String::as_str), Some("1"));
        assert_eq!(
            resolved.scripts.get("test"),
            Some(&vec!["incan".to_string(), "test".to_string(), "--unit".to_string()])
        );
        assert_eq!(
            resolved.scripts.get("docs_build"),
            Some(&vec![
                "python3".to_string(),
                "-m".to_string(),
                "mkdocs".to_string(),
                "build".to_string(),
                "-q".to_string()
            ])
        );
        Ok(())
    }

    #[test]
    fn detached_env_skips_implicit_default() -> Result<(), Box<dyn std::error::Error>> {
        let config = parse_config(
            r#"
            [tool.incan.envs.default]
            env-vars = { DEFAULT_ONLY = "1" }

            [tool.incan.envs.clean]
            detached = true
            env-vars = { CLEAN_ONLY = "1" }
            "#,
        )?;

        let resolved = config.resolve_env("clean")?;

        assert_eq!(resolved.overlay_chain, vec![PROJECT_BASE_OVERLAY, "clean"]);
        assert!(!resolved.env_vars.contains_key("DEFAULT_ONLY"));
        assert_eq!(resolved.env_vars.get("CLEAN_ONLY").map(String::as_str), Some("1"));
        Ok(())
    }

    #[test]
    fn duplicate_inclusion_is_reported() -> Result<(), Box<dyn std::error::Error>> {
        let config = parse_config(
            r#"
            [tool.incan.envs.default]

            [tool.incan.envs.unit]
            extends = ["default"]
            "#,
        )?;

        let error = match config.resolve_env("unit") {
            Ok(resolved) => return Err(format!("expected duplicate error, resolved {resolved:?}").into()),
            Err(error) => error,
        };

        assert_eq!(
            error,
            EnvConfigError::DuplicateInclusion {
                name: "default".to_string(),
                chain: vec![
                    PROJECT_BASE_OVERLAY.to_string(),
                    "default".to_string(),
                    "default".to_string()
                ],
            }
        );
        Ok(())
    }

    #[test]
    fn duplicate_inclusion_through_shared_parent_is_reported() -> Result<(), Box<dyn std::error::Error>> {
        let config = parse_config(
            r#"
            [tool.incan.envs.shared]

            [tool.incan.envs.a]
            extends = ["shared"]

            [tool.incan.envs.b]
            extends = ["shared"]

            [tool.incan.envs.target]
            detached = true
            extends = ["a", "b"]
            "#,
        )?;

        let error = match config.resolve_env("target") {
            Ok(resolved) => return Err(format!("expected duplicate error, resolved {resolved:?}").into()),
            Err(error) => error,
        };

        assert!(matches!(
            error,
            EnvConfigError::DuplicateInclusion { name, .. } if name == "shared"
        ));
        Ok(())
    }

    #[test]
    fn cycles_are_reported_with_cycle_path() -> Result<(), Box<dyn std::error::Error>> {
        let config = parse_config(
            r#"
            [tool.incan.envs.a]
            detached = true
            extends = ["b"]

            [tool.incan.envs.b]
            extends = ["c"]

            [tool.incan.envs.c]
            extends = ["a"]
            "#,
        )?;

        let error = match config.resolve_env("a") {
            Ok(resolved) => return Err(format!("expected cycle error, resolved {resolved:?}").into()),
            Err(error) => error,
        };

        assert_eq!(
            error,
            EnvConfigError::Cycle {
                cycle: vec!["a".to_string(), "b".to_string(), "c".to_string(), "a".to_string()],
            }
        );
        Ok(())
    }

    #[test]
    fn base_overlay_and_env_dependencies_merge_deterministically() -> Result<(), Box<dyn std::error::Error>> {
        let base = EnvOverlay {
            dependencies: BTreeMap::from([(
                "serde".to_string(),
                r#"{ version = "1.0", features = ["derive", "std"], default-features = false }"#.parse::<Value>()?,
            )]),
            ..EnvOverlay::default()
        };

        let config = parse_config(
            r#"
            [tool.incan.envs.unit.dependencies.serde]
            version = "1.1"
            features = ["alloc", "derive"]

            [tool.incan.envs.unit.dev-dependencies.proptest]
            version = "1"
            "#,
        )?
        .with_base_overlay(base);

        let resolved = config.resolve_env("unit")?;
        let Some(Value::Table(serde)) = resolved.dependencies.get("serde") else {
            return Err("expected serde dependency table".into());
        };

        assert_eq!(serde.get("version").and_then(Value::as_str), Some("1.1"));
        assert_eq!(serde.get("default-features").and_then(Value::as_bool), Some(false));
        assert_eq!(
            serde.get("features").and_then(Value::as_array),
            Some(&vec![
                Value::String("alloc".to_string()),
                Value::String("derive".to_string()),
                Value::String("std".to_string()),
            ])
        );
        assert!(resolved.dev_dependencies.contains_key("proptest"));
        Ok(())
    }

    #[test]
    fn rust_dependency_aliases_parse_to_dependency_overlays() -> Result<(), Box<dyn std::error::Error>> {
        let config = parse_config(
            r#"
            [tool.incan.envs.unit.rust-dependencies.serde]
            version = "1"

            [tool.incan.envs.unit.rust-dev-dependencies.proptest]
            version = "1"
            "#,
        )?;

        let resolved = config.resolve_env("unit")?;

        assert!(resolved.dependencies.contains_key("serde"));
        assert!(resolved.dev_dependencies.contains_key("proptest"));
        Ok(())
    }

    #[test]
    fn duplicate_dependency_table_spellings_are_parse_errors() -> Result<(), Box<dyn std::error::Error>> {
        let error = match ProjectManifest::from_str(
            r#"
            [tool.incan.envs.unit.dependencies.serde]
            version = "1"

            [tool.incan.envs.unit.rust-dependencies.serde]
            version = "1"
            "#,
            Path::new("incan.toml"),
        ) {
            Ok(manifest) => return Err(format!("expected parse error, parsed {manifest:?}").into()),
            Err(error) => error,
        };

        let rendered = error.to_string();
        assert!(
            rendered.contains("dependencies") && rendered.contains("rust-dependencies"),
            "expected duplicate dependency table spelling diagnostic, got: {rendered}"
        );
        Ok(())
    }

    #[test]
    fn dry_run_preview_resolves_cwd_and_appends_passthrough_args() -> Result<(), Box<dyn std::error::Error>> {
        let config = parse_config(
            r#"
            [tool.incan.envs.unit]
            cwd = "tests"
            env-vars = { INCAN_NO_BANNER = "1" }

            [tool.incan.envs.unit.scripts]
            test = ["incan", "test"]
            "#,
        )?;

        let preview = config.resolve_run_preview(
            Path::new("/project"),
            "unit",
            "test",
            &["--filter".to_string(), "addition".to_string()],
        )?;

        assert_eq!(preview.cwd, Path::new("/project/tests"));
        assert_eq!(
            preview.argv,
            vec![
                "incan".to_string(),
                "test".to_string(),
                "--filter".to_string(),
                "addition".to_string(),
            ]
        );
        assert_eq!(preview.env_vars.get("INCAN_NO_BANNER").map(String::as_str), Some("1"));
        Ok(())
    }

    #[test]
    fn resolves_project_and_env_toolchain_constraints_in_overlay_order() -> Result<(), Box<dyn std::error::Error>> {
        let config = parse_config(
            r#"
            [project]
            requires-incan = ">=0.3,<0.5"

            [tool.incan.envs.default]
            requires-incan = ">=0.3,<0.4"

            [tool.incan.envs.release]
            requires-incan = ">=0.3.1,<0.4"
            "#,
        )?;

        let resolved = config.resolve_env("release")?;

        assert_eq!(
            resolved
                .requires_incan
                .layers()
                .iter()
                .map(|layer| (layer.source.as_str(), layer.requirement.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("project.requires-incan", ">=0.3,<0.5"),
                ("env.default.requires-incan", ">=0.3,<0.4"),
                ("env.release.requires-incan", ">=0.3.1,<0.4"),
            ]
        );
        Ok(())
    }

    #[test]
    fn malformed_project_requires_incan_is_manifest_error() -> Result<(), Box<dyn std::error::Error>> {
        let error = match ProjectManifest::from_str(
            r#"
            [project]
            requires-incan = "not semver"
            "#,
            Path::new("incan.toml"),
        ) {
            Ok(manifest) => return Err(format!("expected manifest error, parsed {manifest:?}").into()),
            Err(error) => error,
        };

        assert!(
            error.to_string().contains("invalid requires-incan constraint"),
            "expected requires-incan parse diagnostic, got: {error}"
        );
        Ok(())
    }

    #[test]
    fn dependency_string_spec_replaces_base_table_spec() -> Result<(), Box<dyn std::error::Error>> {
        let base = r#"{ version = "1.0", features = ["derive"] }"#.parse::<Value>()?;
        let overlay = Value::String("2.0".to_string());

        assert_eq!(merge_dependency_value(&base, &overlay), overlay);
        Ok(())
    }

    #[test]
    fn unknown_env_keys_are_parse_errors() -> Result<(), Box<dyn std::error::Error>> {
        let error = match ProjectManifest::from_str(
            r#"
            [tool.incan.envs.unit]
            env-varz = {}
            "#,
            Path::new("incan.toml"),
        ) {
            Ok(manifest) => return Err(format!("expected parse error, parsed {manifest:?}").into()),
            Err(error) => error,
        };

        assert!(error.to_string().contains("unknown field `env-varz`"));
        Ok(())
    }

    fn assert_resolved_env_is_send_sync<T: Send + Sync>() {}

    #[test]
    fn resolved_env_is_plain_data() {
        assert_resolved_env_is_send_sync::<ResolvedEnv>();
    }
}
