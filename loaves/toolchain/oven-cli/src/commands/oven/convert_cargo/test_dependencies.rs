//! Cargo adoption keeps upstream test requirements in the single Loaf environment model.

use super::convert;
use oven_model::manifest::ProjectManifest;
use oven_model::project_lifecycle::env::EnvConfigSet;

/// Construct a workspace with ordinary, optional, local and conditional test edges.
fn workspace() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    std::fs::write(
        root.path().join("Cargo.toml"),
        r#"
[workspace.package]
version = "1.0.0"
edition = "2024"
[workspace.dependencies]
serde = { version = "1", default-features = false, features = ["alloc"] }
helper = { path = "helper" }
"#,
    )?;
    std::fs::write(
        root.path().join("Cargo.lock"),
        "version=4\n[[package]]\nname='serde'\nversion='1.0.228'\nsource='registry+https://github.com/rust-lang/crates.io-index'\n",
    )?;
    for name in ["subject", "helper"] {
        std::fs::create_dir_all(root.path().join(name).join("src"))?;
        std::fs::write(root.path().join(name).join("src/lib.rs"), "")?;
        std::fs::write(
            root.path().join(name).join("Cargo.toml"),
            format!("[package]\nname='{name}'\nversion.workspace=true\nedition.workspace=true\n"),
        )?;
    }
    Ok(root)
}

/// Optional normal edges become required only for tests, local dev packages are adopted, and reimports converge.
#[test]
fn adopts_test_dependencies_without_changing_production_requirements() -> Result<(), Box<dyn std::error::Error>> {
    let root = workspace()?;
    let cargo_path = root.path().join("subject/Cargo.toml");
    let original = std::fs::read_to_string(&cargo_path)?
        + r#"
[dependencies]
serde = { workspace = true, optional = true }
[dev-dependencies]
serde = { workspace = true, features = ["derive"] }
helper.workspace = true
[target.'cfg(unix)'.dev-dependencies]
unix_serde = { package = "serde", version = "1" }
"#;
    std::fs::write(&cargo_path, &original)?;
    convert(root.path(), &["subject".into()])?;
    let path = root.path().join("subject/loaf.toml");
    let first = std::fs::read(&path)?;
    let manifest = ProjectManifest::load(&path)?;
    let env = EnvConfigSet::from_manifest(&manifest);
    let production = manifest.with_environment_dependencies(&env.resolve_env("default")?.dependencies)?;
    let testing = manifest.with_environment_dependencies(&env.resolve_env("test")?.dependencies)?;
    assert!(production.rust_dependencies()["serde"].optional);
    assert!(!testing.rust_dependencies()["serde"].optional);
    assert!(!testing.rust_dependencies()["serde"].default_features);
    assert_eq!(testing.rust_dependencies()["serde"].features, ["alloc", "derive"]);
    assert!(!production.rust_dependencies().contains_key("helper"));
    assert!(testing.rust_dependencies().contains_key("helper"));
    assert!(root.path().join("helper/loaf.toml").is_file());
    let rendered: toml::Value = toml::from_str(&std::fs::read_to_string(&path)?)?;
    assert_eq!(
        rendered["envs"]["test"]["dependencies"]["unix_serde"]["target"].as_str(),
        Some("cfg(unix)")
    );
    assert_eq!(std::fs::read_to_string(&cargo_path)?, original);
    convert(root.path(), &["subject".into()])?;
    assert_eq!(std::fs::read(&path)?, first);
    Ok(())
}

/// Removing a dev declaration removes its imported environment edge instead of retaining stale requirements.
#[test]
fn readoption_removes_deleted_test_dependencies() -> Result<(), Box<dyn std::error::Error>> {
    let root = workspace()?;
    let path = root.path().join("subject/Cargo.toml");
    let original = std::fs::read_to_string(&path)?;
    std::fs::write(
        &path,
        original.clone() + "\n[dev-dependencies]\nhelper.workspace=true\n",
    )?;
    convert(root.path(), &["subject".into()])?;
    std::fs::write(&path, original)?;
    convert(root.path(), &["subject".into()])?;
    let manifest = ProjectManifest::load(&root.path().join("subject/loaf.toml"))?;
    let testing = EnvConfigSet::from_manifest(&manifest).resolve_env("test")?;
    assert!(!testing.dependencies.contains_key("helper"));
    Ok(())
}

/// Missing test authority or an unrepresentable optional feature merge fails before publishing any manifest.
#[test]
fn unsupported_test_dependencies_refuse_before_publication() -> Result<(), Box<dyn std::error::Error>> {
    for (dependency, diagnostic) in [
        (
            "[dev-dependencies]\nprobe={git='https://example.invalid/probe'}\n",
            "explicit registered Loaf adaptation",
        ),
        (
            "[dependencies]\nserde={version='1',optional=true,features=['derive']}\n[dev-dependencies]\nserde={version='1',default-features=false}\n",
            "optional production-only features",
        ),
    ] {
        let root = workspace()?;
        let path = root.path().join("subject/Cargo.toml");
        std::fs::write(&path, std::fs::read_to_string(&path)? + dependency)?;
        let error = convert(root.path(), &["subject".into()])
            .err()
            .ok_or("unsupported test requirement accepted")?;
        assert!(error.to_string().contains(diagnostic), "{error}");
        assert!(!root.path().join("subject/loaf.toml").exists());
    }
    Ok(())
}
