//! Translate Cargo's implicit optional-dependency features into the explicit Loaf feature graph.

use super::Error;
use std::collections::BTreeSet;
use toml::{Value, map::Map};

/// Preserve declared features and expose optional aliases unless an explicit `dep:` edge suppresses them.
/// Rebuild from the adoption input on every import so deleted implicit features cannot survive in the Loaf.
pub(super) fn convert(cargo: &Value, loaf: &mut Value, dependencies: &Map<String, Value>) -> Result<(), Error> {
    let mut features = match cargo.get("features") {
        Some(value) => value.as_table().ok_or("Cargo features must be a table")?.clone(),
        None => Map::new(),
    };
    let mut private = BTreeSet::new();
    for members in features.values() {
        for member in members.as_array().ok_or("Cargo feature members must be an array")? {
            let member = member.as_str().ok_or("Cargo feature member must be a string")?;
            if let Some(alias) = member.strip_prefix("dep:") {
                private.insert(alias.to_string());
            }
        }
    }
    for (alias, dependency) in dependencies {
        if optional(dependency) && !private.contains(alias) {
            features
                .entry(alias.clone())
                .or_insert_with(|| Value::Array(vec![Value::String(format!("dep:{alias}"))]));
        }
    }
    let project = loaf
        .get_mut("project")
        .and_then(Value::as_table_mut)
        .ok_or("Loaf project must be a table")?;
    project.insert("features".to_string(), Value::Table(features));
    Ok(())
}

/// A conditional alias exposes the same implicit feature if any of its declarations is optional.
fn optional(dependency: &Value) -> bool {
    match dependency {
        Value::Array(alternatives) => alternatives.iter().any(optional),
        Value::Table(entry) => entry.get("optional").and_then(Value::as_bool) == Some(true),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::super::convert;
    use incan_provider::{FeatureSelection, PackageFeatureGraph};
    use oven_model::manifest::ProjectManifest;

    /// Exercise the public adoption transaction before resolving the resulting ordinary Loaf graph.
    fn fixture(features: &str) -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        std::fs::create_dir_all(root.path().join("subject/src"))?;
        std::fs::write(root.path().join("Cargo.toml"), "[workspace]\nmembers=['subject']\n")?;
        std::fs::write(
            root.path().join("Cargo.lock"),
            "version=4\n[[package]]\nname='serde'\nversion='1.0.228'\nsource='registry+https://github.com/rust-lang/crates.io-index'\n",
        )?;
        std::fs::write(root.path().join("subject/src/lib.rs"), "")?;
        std::fs::write(
            root.path().join("subject/Cargo.toml"),
            format!(
                "[package]\nname='subject'\nversion='1.0.0'\n[dependencies]\nserde={{version='1',optional=true}}\n{features}"
            ),
        )?;
        convert(root.path(), &["subject".into()])?;
        Ok(root)
    }

    /// Cargo's bare optional alias remains an exposed feature and an inclusion target after adoption.
    #[test]
    fn implicit_optional_alias_uses_canonical_feature_resolution() -> Result<(), Box<dyn std::error::Error>> {
        let root = fixture("[features]\nserialize=['serde']\n")?;
        let manifest = ProjectManifest::load(&root.path().join("subject/loaf.toml"))?;
        let graph = PackageFeatureGraph::from_manifest(&manifest)?;
        assert!(
            graph
                .resolve(&FeatureSelection::default())?
                .active_optional_dependencies
                .is_empty()
        );
        for requested in ["serde", "serialize"] {
            assert!(
                graph
                    .resolve(&FeatureSelection::new([requested]))?
                    .active_optional_dependencies
                    .contains("serde")
            );
        }
        Ok(())
    }

    /// An explicit dep edge hides the implicit alias; an explicitly authored feature of that name stays valid.
    #[test]
    fn explicit_dependency_edges_preserve_cargo_visibility() -> Result<(), Box<dyn std::error::Error>> {
        for (features, exposes_alias) in [
            ("[features]\nserialize=['dep:serde']\n", false),
            ("[features]\nserde=['dep:serde']\nserialize=['serde']\n", true),
        ] {
            let root = fixture(features)?;
            let manifest = ProjectManifest::load(&root.path().join("subject/loaf.toml"))?;
            let graph = PackageFeatureGraph::from_manifest(&manifest)?;
            assert_eq!(graph.resolve(&FeatureSelection::new(["serde"])).is_ok(), exposes_alias);
            assert!(
                graph
                    .resolve(&FeatureSelection::new(["serialize"]))?
                    .active_optional_dependencies
                    .contains("serde")
            );
        }
        Ok(())
    }

    /// Reimport removes a formerly implicit feature when its dependency becomes mandatory.
    #[test]
    fn reimport_replaces_implicit_feature_inventory() -> Result<(), Box<dyn std::error::Error>> {
        let root = fixture("")?;
        let path = root.path().join("subject/Cargo.toml");
        std::fs::write(&path, std::fs::read_to_string(&path)?.replace(",optional=true", ""))?;
        convert(root.path(), &["subject".into()])?;
        let manifest = ProjectManifest::load(&root.path().join("subject/loaf.toml"))?;
        let graph = PackageFeatureGraph::from_manifest(&manifest)?;
        assert!(graph.resolve(&FeatureSelection::new(["serde"])).is_err());
        assert!(graph.resolve(&FeatureSelection::default()).is_ok());
        Ok(())
    }
}
