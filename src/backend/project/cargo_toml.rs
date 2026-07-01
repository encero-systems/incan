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

use std::path::{Path, PathBuf};
use std::{fs, io};

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

/// Return the dependency key that generated Cargo manifests should render for `spec`.
///
/// Cargo lets a dependency key differ from the published package name via `package = "..."`. Generated manifests use
/// the Rust-import-safe dependency key here so the key matches the crate spelling that downstream code and
/// rust-inspect queries use.
fn rendered_dependency_key(spec: &DependencySpec) -> String {
    spec.crate_name.replace('-', "_")
}

/// Return the published Cargo package name to render for `spec`, if it differs from the dependency key.
fn rendered_package_name(spec: &DependencySpec, dependency_key: &str) -> Option<String> {
    spec.package.clone().or_else(|| {
        if dependency_key != spec.crate_name {
            Some(spec.crate_name.clone())
        } else {
            None
        }
    })
}

/// Convert a [`DependencySpec`] into a `(dependency_key, toml::Value)` pair.
///
/// Returns [`toml::Value::String`] for simple version-only registry deps (shorthand form),
/// or [`toml::Value::Table`] for deps that need extra fields (features, git, path, etc.).
fn dependency_spec_to_toml(spec: &DependencySpec, output_dir: &Path) -> (String, toml::Value) {
    let dependency_key = rendered_dependency_key(spec);
    let package_name = rendered_package_name(spec, &dependency_key);

    // ---- Check if shorthand form is possible ----
    let shorthand_ok = matches!(spec.source, DependencySource::Registry)
        && spec.version.is_some()
        && spec.features.is_empty()
        && spec.default_features
        && !spec.optional
        && package_name.is_none();

    if shorthand_ok {
        let version = spec.version.as_deref().unwrap_or("*");
        return (dependency_key, toml::Value::String(version.to_string()));
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

    if let Some(package) = package_name {
        table.insert("package".into(), package.into());
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

    (dependency_key, toml::Value::Table(table))
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

fn installed_toolchain_crate_path(crate_name: &str) -> Option<PathBuf> {
    let exe_path = std::env::current_exe().ok()?;
    let canonical_exe_path = fs::canonicalize(&exe_path).ok();

    let mut exe_paths = vec![exe_path];
    if let Some(canonical) = canonical_exe_path {
        exe_paths.push(canonical);
    }

    for exe_path in exe_paths {
        let Some(exe_dir) = exe_path.parent() else {
            continue;
        };
        for base in [
            Some(exe_dir),
            exe_dir.parent(),
            exe_dir.parent().and_then(|p| p.parent()),
        ]
        .into_iter()
        .flatten()
        {
            let candidate = base.join("crates").join(crate_name);
            if candidate.join("Cargo.toml").exists() {
                return Some(candidate);
            }
        }
    }

    None
}

fn toolchain_crate_path(crate_name: &str) -> PathBuf {
    installed_toolchain_crate_path(crate_name).unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("crates")
            .join(crate_name)
    })
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
        let package_name = self.package_name.as_deref().unwrap_or(&self.name).to_string();

        // ---- Resolve toolchain-owned support crates for generated Rust projects ----
        let stdlib_path = toolchain_crate_path("incan_stdlib");
        let derive_path = toolchain_crate_path("incan_derive");

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
            let dependency_key = rendered_dependency_key(spec);
            if added_crates.contains(&dependency_key) {
                continue;
            }
            let (name, value) = dependency_spec_to_toml(spec, &self.output_dir);
            if spec.optional {
                optional_features.push(name.clone());
            }
            added_crates.insert(name.clone());
            deps.insert(name, value);
        }

        // ---- Build dev-dependencies table ----
        let dev_deps = if self.include_dev_dependencies && !self.dev_dependencies.is_empty() {
            let mut dev = toml::Table::new();
            for spec in &self.dev_dependencies {
                let dependency_key = rendered_dependency_key(spec);
                if added_crates.contains(&dependency_key) {
                    continue;
                }
                let (name, value) = dependency_spec_to_toml(spec, &self.output_dir);
                if spec.optional {
                    optional_features.push(name.clone());
                }
                added_crates.insert(name.clone());
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
        let target_name = self.cargo_target_name();
        let (bin, lib) = if self.is_binary {
            (
                vec![BinTarget {
                    name: target_name,
                    path: "src/main.rs".into(),
                }],
                None,
            )
        } else {
            (
                vec![],
                Some(LibTarget {
                    name: target_name,
                    path: "src/lib.rs".into(),
                }),
            )
        };

        // ---- Assemble and serialize ----
        let manifest = CargoManifest {
            package: PackageSection {
                name: package_name,
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
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    use crate::backend::project::generator::ProjectGenerator;
    use crate::manifest::{DependencySource, DependencySpec};

    fn parsed_manifest(toml: &str) -> Result<toml::Value, Box<dyn std::error::Error>> {
        Ok(toml::from_str(toml)?)
    }

    fn dependency_keys(toml: &str) -> Result<BTreeSet<String>, Box<dyn std::error::Error>> {
        let manifest = parsed_manifest(toml)?;
        let dependencies = manifest
            .get("dependencies")
            .and_then(toml::Value::as_table)
            .ok_or("generated Cargo.toml missing [dependencies] table")?;

        Ok(dependencies.keys().cloned().collect())
    }

    fn stdlib_features(toml: &str) -> Result<BTreeSet<String>, Box<dyn std::error::Error>> {
        let manifest = parsed_manifest(toml)?;
        let features = manifest
            .get("dependencies")
            .and_then(toml::Value::as_table)
            .and_then(|dependencies| dependencies.get("incan_stdlib"))
            .and_then(toml::Value::as_table)
            .and_then(|stdlib| stdlib.get("features"))
            .and_then(toml::Value::as_array)
            .map(|features| {
                features
                    .iter()
                    .filter_map(toml::Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default();

        Ok(features)
    }

    fn assert_dependency_contract(
        toml: &str,
        expected_dependencies: &[&str],
        expected_stdlib_features: &[&str],
        forbidden_dependencies: &[&str],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let actual_dependencies = dependency_keys(toml)?;
        let expected_dependencies = expected_dependencies.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            actual_dependencies, expected_dependencies,
            "generated Cargo.toml dependencies should match the contract, got:\n{toml}"
        );

        let actual_stdlib_features = stdlib_features(toml)?;
        let expected_stdlib_features = expected_stdlib_features
            .iter()
            .map(|feature| (*feature).to_string())
            .collect();
        assert_eq!(
            actual_stdlib_features, expected_stdlib_features,
            "incan_stdlib features should match the generated-project contract, got:\n{toml}"
        );

        let actual_dependencies = dependency_keys(toml)?;
        for forbidden in forbidden_dependencies {
            assert!(
                !actual_dependencies.contains(*forbidden),
                "generated Cargo.toml should not expose `{forbidden}` as a direct dependency, got:\n{toml}"
            );
        }

        Ok(())
    }

    fn dependency_spec(crate_name: &str, version: &str, features: &[&str]) -> DependencySpec {
        DependencySpec {
            crate_name: crate_name.to_string(),
            version: Some(version.to_string()),
            features: features.iter().map(|feature| (*feature).to_string()).collect(),
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: None,
        }
    }

    #[test]
    fn test_cargo_toml_generation() -> Result<(), Box<dyn std::error::Error>> {
        let generator = ProjectGenerator::new("/tmp/test", "hello", true);
        let toml = generator.generate_cargo_toml()?;
        assert!(toml.contains("name = \"hello\""));
        assert!(toml.contains("[[bin]]"));
        Ok(())
    }

    #[test]
    fn cargo_toml_supports_distinct_package_and_target_names() -> Result<(), Box<dyn std::error::Error>> {
        let mut generator = ProjectGenerator::new("/tmp/test_runner", "test_runner_abc", false);
        generator.set_package_name(Some("locked_root".to_string()));
        let toml = generator.generate_cargo_toml()?;

        assert!(toml.contains("[package]\nname = \"locked_root\""));
        assert!(toml.contains("[lib]\nname = \"test_runner_abc\""));
        Ok(())
    }

    #[test]
    fn minimal_generated_manifest_exposes_only_base_direct_crates() -> Result<(), Box<dyn std::error::Error>> {
        let generator = ProjectGenerator::new("/tmp/test_minimal_manifest_contract", "minimal_contract", true);
        let toml = generator.generate_cargo_toml()?;

        assert_dependency_contract(
            &toml,
            &["incan_derive", "incan_stdlib"],
            &[],
            &["axum", "incan_web_macros", "inventory", "serde", "serde_json", "tokio"],
        )
    }

    #[test]
    fn json_generated_manifest_keeps_serde_json_behind_stdlib_feature() -> Result<(), Box<dyn std::error::Error>> {
        let mut generator = ProjectGenerator::new("/tmp/test_json_manifest_contract", "json_contract", true);
        generator.set_stdlib_features(vec!["json".to_string()]);
        generator.set_dependencies(vec![dependency_spec("serde", "1.0", &["derive"])]);
        let toml = generator.generate_cargo_toml()?;

        assert_dependency_contract(
            &toml,
            &["incan_derive", "incan_stdlib", "serde"],
            &["json"],
            &["axum", "incan_web_macros", "inventory", "serde_json", "tokio"],
        )
    }

    #[test]
    fn async_generated_manifest_keeps_tokio_behind_stdlib_feature() -> Result<(), Box<dyn std::error::Error>> {
        let mut generator = ProjectGenerator::new("/tmp/test_async_manifest_contract", "async_contract", true);
        generator.set_stdlib_features(vec!["async".to_string()]);
        let toml = generator.generate_cargo_toml()?;

        assert_dependency_contract(
            &toml,
            &["incan_derive", "incan_stdlib"],
            &["async"],
            &["axum", "incan_web_macros", "inventory", "serde", "serde_json", "tokio"],
        )
    }

    #[test]
    fn web_generated_manifest_bounds_transitional_direct_deps() -> Result<(), Box<dyn std::error::Error>> {
        let mut generator = ProjectGenerator::new("/tmp/test_web_manifest_contract", "web_contract", true);
        generator.set_stdlib_features(vec!["web".to_string()]);
        generator.set_dependencies(vec![
            dependency_spec("axum", "0.8", &[]),
            dependency_spec("inventory", "0.3", &[]),
            DependencySpec {
                crate_name: "incan_web_macros".to_string(),
                version: None,
                features: vec![],
                default_features: true,
                source: DependencySource::Path {
                    path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("crates/incan_web_macros"),
                },
                optional: false,
                package: None,
            },
        ]);
        let toml = generator.generate_cargo_toml()?;

        assert_dependency_contract(
            &toml,
            &["axum", "incan_derive", "incan_stdlib", "incan_web_macros", "inventory"],
            &["web"],
            &["serde", "serde_json", "tokio"],
        )
    }

    // ---- Phase 1: version propagation to Cargo.toml ----

    #[test]
    fn test_cargo_toml_version_propagation() -> Result<(), Box<dyn std::error::Error>> {
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

    #[test]
    fn test_cargo_toml_hyphenated_dependency_uses_rust_safe_key_and_package_alias()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::manifest::{DependencySource, DependencySpec};

        let mut generator = ProjectGenerator::new("/tmp/consumer/out", "consumer", true);
        generator.set_dependencies(vec![DependencySpec {
            crate_name: "datafusion-substrait".to_string(),
            version: Some("53".to_string()),
            features: vec!["protoc".to_string()],
            default_features: true,
            source: DependencySource::Registry,
            optional: true,
            package: None,
        }]);

        let toml = generator.generate_cargo_toml()?;
        assert!(
            toml.contains("[dependencies.datafusion_substrait]"),
            "expected underscore-safe dependency key in Cargo.toml, got:\n{toml}"
        );
        assert!(
            toml.contains("package = \"datafusion-substrait\""),
            "expected original Cargo package name in package field, got:\n{toml}"
        );
        assert!(
            toml.contains("datafusion_substrait = [\"dep:datafusion_substrait\"]"),
            "expected optional feature gate to track the rendered dependency key, got:\n{toml}"
        );
        Ok(())
    }
}
