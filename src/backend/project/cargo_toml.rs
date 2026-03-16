//! Cargo.toml generation and dependency formatting
//!
//! Renders the `Cargo.toml` for generated Rust projects using typed structs and [`toml::to_string`] serialization. This
//! replaces manual string formatting with structured data construction, ensuring valid TOML output by construction.
//!
//! ## Output format
//!
//! The serializer produces standard Cargo.toml layout:
//! - Simple version deps as inline strings: `serde = "1.0"`
//! - Complex deps as subsections: `[dependencies.tokio]` with version, features, etc.
//! - Both forms are semantically equivalent and understood by cargo.

use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::manifest::{DependencySource, DependencySpec, GitReference};

use super::generator::ProjectGenerator;

/// Incan compiler version stamped into generated `Cargo.toml` files.
pub(crate) const INCAN_VERSION: &str = crate::version::INCAN_VERSION;

// ============================================================================
// Serializable Cargo.toml structure
// ============================================================================

/// Top-level Cargo.toml document.
///
/// Field order determines section order in the serialized output.
#[derive(Serialize)]
struct CargoManifest {
    package: PackageSection,
    workspace: toml::Table,
    dependencies: toml::Table,
    #[serde(rename = "dev-dependencies", skip_serializing_if = "Option::is_none")]
    dev_dependencies: Option<toml::Table>,
    #[serde(skip_serializing_if = "Option::is_none")]
    features: Option<toml::Table>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    bin: Vec<BinTarget>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lib: Option<LibTarget>,
}

#[derive(Serialize)]
struct PackageSection {
    name: String,
    version: String,
    edition: String,
}

#[derive(Serialize)]
struct BinTarget {
    name: String,
    path: String,
}

#[derive(Serialize)]
struct LibTarget {
    name: String,
    path: String,
}

// ============================================================================
// Dependency conversion
// ============================================================================

/// Convert a [`DependencySpec`] into a `(crate_name, toml::Value)` pair.
///
/// Returns [`toml::Value::String`] for simple version-only registry deps (shorthand form),
/// or [`toml::Value::Table`] for deps that need extra fields (features, git, path, etc.).
fn dependency_spec_to_toml(spec: &DependencySpec, output_dir: &Path) -> (String, toml::Value) {
    // ---- Check if shorthand form is possible ----
    let shorthand_ok = matches!(spec.source, DependencySource::Registry)
        && spec.version.is_some()
        && spec.features.is_empty()
        && spec.default_features
        && !spec.optional
        && spec.package.is_none();

    if shorthand_ok {
        let version = spec.version.as_deref().unwrap_or("*");
        return (spec.crate_name.clone(), toml::Value::String(version.to_string()));
    }

    // ---- Build a table for complex deps ----
    let mut table = toml::Table::new();

    match &spec.source {
        DependencySource::Registry => {}
        DependencySource::Git { url, reference } => {
            table.insert("git".into(), url.clone().into());
            match reference {
                GitReference::Branch(branch) => {
                    table.insert("branch".into(), branch.clone().into());
                }
                GitReference::Tag(tag) => {
                    table.insert("tag".into(), tag.clone().into());
                }
                GitReference::Rev(rev) => {
                    table.insert("rev".into(), rev.clone().into());
                }
            }
        }
        DependencySource::Path { path } => {
            // Resolve symlinked path aliases (e.g. `/var` -> `/private/var` on macOS) before computing a relative path.
            // Without this, generated path dependencies can point at non-existent siblings like `/private/Users/...`
            // in integration test temp dirs.
            let from = output_dir.canonicalize().unwrap_or_else(|_| output_dir.to_path_buf());
            let to = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
            let rel = relative_path(&from, &to);
            let path_str = rel.to_string_lossy().replace('\\', "/");
            table.insert("path".into(), path_str.into());
        }
    }

    if let Some(package) = &spec.package {
        table.insert("package".into(), package.clone().into());
    }
    if let Some(version) = &spec.version {
        table.insert("version".into(), version.clone().into());
    }
    if !spec.default_features {
        table.insert("default-features".into(), false.into());
    }
    if !spec.features.is_empty() {
        let features: Vec<toml::Value> = spec.features.iter().map(|f| f.clone().into()).collect();
        table.insert("features".into(), toml::Value::Array(features));
    }
    if spec.optional {
        table.insert("optional".into(), true.into());
    }

    (spec.crate_name.clone(), toml::Value::Table(table))
}

/// Build a [`toml::Value::Table`] for a path-only dependency (used for stdlib/derive crates).
fn path_dependency(path: &Path, features: &[String]) -> toml::Value {
    let mut table = toml::Table::new();
    table.insert("path".into(), path.display().to_string().into());
    if !features.is_empty() {
        let feat_values: Vec<toml::Value> = features.iter().map(|f| f.clone().into()).collect();
        table.insert("features".into(), toml::Value::Array(feat_values));
    }
    toml::Value::Table(table)
}

// ============================================================================
// ProjectGenerator impl — Cargo.toml generation
// ============================================================================

impl ProjectGenerator {
    /// Generate Cargo.toml content as a TOML-serialized string.
    ///
    /// Builds a [`CargoManifest`] struct from the generator's configuration and serializes it via [`toml::to_string`].
    /// The output is valid TOML by construction.
    pub(super) fn generate_cargo_toml(&self) -> io::Result<String> {
        let edition = self.rust_edition.as_deref().unwrap_or("2021").to_string();

        // ---- Resolve workspace-rooted paths for internal crates ----
        let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let stdlib_path = workspace_root.join("crates/incan_stdlib");
        let derive_path = workspace_root.join("crates/incan_derive");

        // ---- Build dependencies table ----
        let mut deps = toml::Table::new();
        let mut added_crates: std::collections::HashSet<String> = std::collections::HashSet::new();

        // Always add incan_stdlib with the resolved feature set.
        let stdlib_features = self.stdlib_features.clone();
        deps.insert("incan_stdlib".into(), path_dependency(&stdlib_path, &stdlib_features));
        added_crates.insert("incan_stdlib".into());

        // Always add incan_derive for derive macros
        deps.insert("incan_derive".into(), path_dependency(&derive_path, &Vec::new()));
        added_crates.insert("incan_derive".into());

        // Add resolved user dependencies
        let mut optional_features = Vec::new();
        for spec in &self.dependencies {
            if added_crates.contains(&spec.crate_name) {
                continue;
            }
            let (name, value) = dependency_spec_to_toml(spec, &self.output_dir);
            if spec.optional {
                optional_features.push(spec.crate_name.clone());
            }
            added_crates.insert(name.clone());
            deps.insert(name, value);
        }

        // ---- Build dev-dependencies table ----
        let dev_deps = if self.include_dev_dependencies && !self.dev_dependencies.is_empty() {
            let mut dev = toml::Table::new();
            for spec in &self.dev_dependencies {
                if added_crates.contains(&spec.crate_name) {
                    continue;
                }
                let (name, value) = dependency_spec_to_toml(spec, &self.output_dir);
                if spec.optional {
                    optional_features.push(spec.crate_name.clone());
                }
                dev.insert(name, value);
            }
            if dev.is_empty() { None } else { Some(dev) }
        } else {
            None
        };

        // ---- Build features table ----
        let features = if optional_features.is_empty() {
            None
        } else {
            let mut features_table = toml::Table::new();
            optional_features.sort();
            optional_features.dedup();
            for name in optional_features {
                let gate = toml::Value::Array(vec![format!("dep:{name}").into()]);
                features_table.insert(name, gate);
            }
            Some(features_table)
        };

        // ---- Build bin/lib target ----
        let (bin, lib) = if self.is_binary {
            (
                vec![BinTarget {
                    name: self.name.clone(),
                    path: "src/main.rs".into(),
                }],
                None,
            )
        } else {
            (
                vec![],
                Some(LibTarget {
                    name: self.name.clone(),
                    path: "src/lib.rs".into(),
                }),
            )
        };

        // ---- Assemble and serialize ----
        let manifest = CargoManifest {
            package: PackageSection {
                name: self.name.clone(),
                version: INCAN_VERSION.to_string(),
                edition,
            },
            workspace: toml::Table::new(), // empty — opt out of parent workspace
            dependencies: deps,
            dev_dependencies: dev_deps,
            features,
            bin,
            lib,
        };

        let body =
            toml::to_string(&manifest).map_err(|e| io::Error::other(format!("failed to serialize Cargo.toml: {e}")))?;

        Ok(format!("# Generated by the Incan compiler v{INCAN_VERSION}\n\n{body}"))
    }
}

// ============================================================================
// Helpers
// ============================================================================

/// Compute a relative path from `from` to `to`.
///
/// Used to make path dependencies relative to the output directory so generated `Cargo.toml` files remain relocatable.
fn relative_path(from: &Path, to: &Path) -> PathBuf {
    let from_components: Vec<_> = from.components().collect();
    let to_components: Vec<_> = to.components().collect();

    let mut common = 0usize;
    for (a, b) in from_components.iter().zip(&to_components) {
        if a == b {
            common += 1;
        } else {
            break;
        }
    }

    let mut result = PathBuf::new();
    for _ in common..from_components.len() {
        result.push("..");
    }
    for comp in &to_components[common..] {
        result.push(comp.as_os_str());
    }

    if result.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        result
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::backend::project::generator::ProjectGenerator;

    #[test]
    fn test_cargo_toml_generation() -> Result<(), Box<dyn std::error::Error>> {
        let generator = ProjectGenerator::new("/tmp/test", "hello", true);
        let toml = generator.generate_cargo_toml()?;
        assert!(toml.contains("name = \"hello\""));
        assert!(toml.contains("[[bin]]"));
        Ok(())
    }

    // ---- Phase 1: version propagation to Cargo.toml ----

    #[test]
    fn test_cargo_toml_version_propagation() -> Result<(), Box<dyn std::error::Error>> {
        use crate::manifest::{DependencySource, DependencySpec};

        let mut generator = ProjectGenerator::new("/tmp/test_ver", "test_ver", true);
        generator.set_dependencies(vec![
            DependencySpec {
                crate_name: "serde".to_string(),
                version: Some("1.0".to_string()),
                features: vec![],
                default_features: true,
                source: DependencySource::Registry,
                optional: false,
                package: None,
            },
            DependencySpec {
                crate_name: "tokio".to_string(),
                version: Some("1.36".to_string()),
                features: vec!["full".to_string(), "macros".to_string()],
                default_features: true,
                source: DependencySource::Registry,
                optional: false,
                package: None,
            },
        ]);

        let toml = generator.generate_cargo_toml()?;

        // Simple version shorthand for serde (no features)
        assert!(
            toml.contains(r#"serde = "1.0""#),
            "Expected serde shorthand version in Cargo.toml, got:\n{toml}"
        );
        // tokio should have version and features (may be in expanded [dependencies.tokio] form)
        assert!(
            toml.contains(r#"version = "1.36""#),
            "Expected tokio version in Cargo.toml, got:\n{toml}"
        );
        assert!(
            toml.contains(r#""full""#) && toml.contains(r#""macros""#),
            "Expected tokio features in Cargo.toml, got:\n{toml}"
        );
        Ok(())
    }

    // ---- Phase 5: git, path, optional deps in Cargo.toml ----

    #[test]
    fn test_cargo_toml_git_dependency() -> Result<(), Box<dyn std::error::Error>> {
        use crate::manifest::{DependencySource, DependencySpec, GitReference};

        let mut generator = ProjectGenerator::new("/tmp/test_git", "test_git", true);
        generator.set_dependencies(vec![DependencySpec {
            crate_name: "my_crate".to_string(),
            version: None,
            features: vec![],
            default_features: true,
            source: DependencySource::Git {
                url: "https://github.com/example/my_crate.git".to_string(),
                reference: GitReference::Tag("v1.0.0".to_string()),
            },
            optional: false,
            package: None,
        }]);

        let toml = generator.generate_cargo_toml()?;
        assert!(
            toml.contains(r#"git = "https://github.com/example/my_crate.git""#),
            "Expected git url in Cargo.toml, got:\n{toml}"
        );
        assert!(
            toml.contains(r#"tag = "v1.0.0""#),
            "Expected git tag in Cargo.toml, got:\n{toml}"
        );
        Ok(())
    }

    #[test]
    fn test_cargo_toml_path_dependency() -> Result<(), Box<dyn std::error::Error>> {
        use crate::manifest::{DependencySource, DependencySpec};

        let mut generator = ProjectGenerator::new("/tmp/test_path", "test_path", true);
        generator.set_dependencies(vec![DependencySpec {
            crate_name: "local_lib".to_string(),
            version: None,
            features: vec![],
            default_features: true,
            source: DependencySource::Path {
                path: PathBuf::from("/home/user/libs/local_lib"),
            },
            optional: false,
            package: None,
        }]);

        let toml = generator.generate_cargo_toml()?;
        assert!(
            toml.contains("path = "),
            "Expected path dependency in Cargo.toml, got:\n{toml}"
        );
        Ok(())
    }

    #[test]
    fn test_cargo_toml_optional_dependency() -> Result<(), Box<dyn std::error::Error>> {
        use crate::manifest::{DependencySource, DependencySpec};

        let mut generator = ProjectGenerator::new("/tmp/test_opt", "test_opt", true);
        generator.set_dependencies(vec![DependencySpec {
            crate_name: "optional_crate".to_string(),
            version: Some("2.0".to_string()),
            features: vec![],
            default_features: true,
            source: DependencySource::Registry,
            optional: true,
            package: None,
        }]);

        let toml = generator.generate_cargo_toml()?;
        assert!(
            toml.contains("optional = true"),
            "Expected optional flag in Cargo.toml, got:\n{toml}"
        );
        assert!(
            toml.contains("[features]"),
            "Expected [features] section for optional dep, got:\n{toml}"
        );
        assert!(
            toml.contains(r#"optional_crate = ["dep:optional_crate"]"#),
            "Expected feature gate for optional dep, got:\n{toml}"
        );
        Ok(())
    }

    #[test]
    fn test_cargo_toml_dev_dependencies_conditional() -> Result<(), Box<dyn std::error::Error>> {
        use crate::manifest::{DependencySource, DependencySpec};

        let dev_dep = DependencySpec {
            crate_name: "test_lib".to_string(),
            version: Some("0.5".to_string()),
            features: vec![],
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: None,
        };

        // Without dev-deps
        let mut gen_no_dev = ProjectGenerator::new("/tmp/test_nodev", "test_nodev", true);
        gen_no_dev.set_dev_dependencies(vec![dev_dep.clone()]);
        gen_no_dev.set_include_dev_dependencies(false);
        let toml_no_dev = gen_no_dev.generate_cargo_toml()?;
        assert!(
            !toml_no_dev.contains("[dev-dependencies]"),
            "build/run should not include dev-dependencies:\n{toml_no_dev}"
        );

        // With dev-deps (test mode)
        let mut gen_with_dev = ProjectGenerator::new("/tmp/test_withdev", "test_withdev", true);
        gen_with_dev.set_dev_dependencies(vec![dev_dep]);
        gen_with_dev.set_include_dev_dependencies(true);
        let toml_with_dev = gen_with_dev.generate_cargo_toml()?;
        assert!(
            toml_with_dev.contains("[dev-dependencies]"),
            "test mode should include dev-dependencies:\n{toml_with_dev}"
        );
        assert!(
            toml_with_dev.contains(r#"test_lib = "0.5""#),
            "Expected dev dep in [dev-dependencies]:\n{toml_with_dev}"
        );
        Ok(())
    }

    #[test]
    fn test_cargo_toml_web_feature_adds_namespace_extra_deps() -> Result<(), Box<dyn std::error::Error>> {
        let mut generator = ProjectGenerator::new("/tmp/test_web_extras", "test_web_extras", true);
        generator.set_stdlib_features(vec!["web".to_string()]);
        generator.set_dependencies(vec![
            crate::manifest::DependencySpec {
                crate_name: "inventory".to_string(),
                version: Some("0.3".to_string()),
                features: vec![],
                default_features: true,
                source: crate::manifest::DependencySource::Registry,
                optional: false,
                package: None,
            },
            crate::manifest::DependencySpec {
                crate_name: "axum".to_string(),
                version: Some("0.8".to_string()),
                features: vec![],
                default_features: true,
                source: crate::manifest::DependencySource::Registry,
                optional: false,
                package: None,
            },
            crate::manifest::DependencySpec {
                crate_name: "incan_web_macros".to_string(),
                version: None,
                features: vec![],
                default_features: true,
                source: crate::manifest::DependencySource::Path {
                    path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("crates/incan_web_macros"),
                },
                optional: false,
                package: None,
            },
        ]);

        let toml = generator.generate_cargo_toml()?;

        assert!(
            toml.contains("incan_web_macros"),
            "Expected incan_web_macros dependency when web feature is enabled, got:\n{toml}"
        );
        assert!(
            toml.contains(r#"inventory = "0.3""#),
            "Expected inventory dependency when web feature is enabled, got:\n{toml}"
        );
        Ok(())
    }

    #[test]
    fn test_cargo_toml_library_path_dependency_with_package_alias() -> Result<(), Box<dyn std::error::Error>> {
        use crate::manifest::{DependencySource, DependencySpec};

        let mut generator = ProjectGenerator::new("/tmp/consumer/out", "consumer", true);
        generator.set_dependencies(vec![DependencySpec {
            crate_name: "widgets".to_string(),
            version: None,
            features: vec![],
            default_features: true,
            source: DependencySource::Path {
                path: PathBuf::from("/tmp/deps/widgets-lib/target/lib"),
            },
            optional: false,
            package: Some("widgets_core".to_string()),
        }]);

        let toml = generator.generate_cargo_toml()?;
        assert!(
            toml.contains("[dependencies.widgets]"),
            "expected expanded table for alias dependency, got:\n{toml}"
        );
        assert!(
            toml.contains("package = \"widgets_core\""),
            "expected package alias in Cargo.toml, got:\n{toml}"
        );
        assert!(
            toml.contains("path = "),
            "expected path-based dependency in Cargo.toml, got:\n{toml}"
        );
        Ok(())
    }
}
