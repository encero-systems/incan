//! Loaf declarations projected into the direct inspector's internal manifest vocabulary.

use std::path::Path;

use crate::error::RustMetadataError;

/// Check that a frozen graph references only retained sources and valid dependency indices before loading it.
///
/// The source authority owns byte integrity. The compiler-authored graph owns feature, fact and domain selection;
/// no manifest or lock is read to rediscover that graph at this boundary.
pub(super) fn validate_frozen_graph(
    graph: &serde_json::Value,
    sources: &[super::OvenInspectionRegistrySource],
    path: &Path,
) -> Result<(), RustMetadataError> {
    let invalid = |message: &str| RustMetadataError::LoadWorkspace {
        path: path.to_path_buf(),
        message: message.to_string(),
    };
    let crates = graph
        .get("crates")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| invalid("sealed Loaf graph has no crate array"))?;
    let roots = sources
        .iter()
        .map(|source| source.source_root.canonicalize())
        .collect::<Result<Vec<_>, _>>()?;
    for record in crates {
        let root = record
            .get("root_module")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| invalid("sealed Loaf graph has no root module"))?;
        let root = Path::new(root).canonicalize()?;
        if !root.is_file() || !roots.iter().any(|source| root.starts_with(source)) {
            return Err(invalid("sealed Loaf graph root escapes retained source authority"));
        }
        let dependencies = record
            .get("deps")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| invalid("sealed Loaf graph has no dependency array"))?;
        let mut aliases = std::collections::BTreeSet::new();
        for dependency in dependencies {
            dependency
                .get("crate")
                .and_then(serde_json::Value::as_u64)
                .and_then(|index| usize::try_from(index).ok())
                .filter(|index| *index < crates.len())
                .ok_or_else(|| invalid("sealed Loaf graph dependency index is invalid"))?;
            let alias = dependency
                .get("name")
                .and_then(serde_json::Value::as_str)
                .filter(|name| !name.is_empty())
                .ok_or_else(|| invalid("sealed Loaf graph dependency has no alias"))?;
            if !aliases.insert(alias) {
                return Err(invalid("sealed Loaf graph repeats a dependency alias"));
            }
        }
    }
    Ok(())
}

/// Read an adopted Loaf without consulting the removed Cargo metadata in its source archive.
///
/// This projection supplies source coordinates only. Exact versions, features and source digests still come from
/// the sealed inspection authority; this reader does not resolve dependencies or execute build scripts.
pub(super) fn read_loaf_manifest(root: &Path) -> Result<toml::Value, RustMetadataError> {
    let path = root.join("loaf.toml");
    let manifest: toml::Value =
        toml::from_str(&std::fs::read_to_string(&path)?).map_err(|error| RustMetadataError::LoadWorkspace {
            path: path.clone(),
            message: format!("invalid inspection Loaf manifest: {error}"),
        })?;
    let project = manifest
        .get("project")
        .ok_or_else(|| RustMetadataError::LoadWorkspace {
            path: path.clone(),
            message: "inspection Loaf has no project declaration".to_string(),
        })?;
    let facet = manifest.get("rust").ok_or_else(|| RustMetadataError::LoadWorkspace {
        path: path.clone(),
        message: "inspection Loaf has no Rust facet".to_string(),
    })?;
    let name = project
        .get("name")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| RustMetadataError::LoadWorkspace {
            path: path.clone(),
            message: "inspection Loaf has no project name".to_string(),
        })?;
    let source = facet
        .get("source")
        .and_then(|source| source.get("root"))
        .and_then(toml::Value::as_str)
        .unwrap_or("src/lib.rs");
    let source = if root.join(source).is_dir() {
        format!("{source}/src/lib.rs")
    } else {
        source.to_string()
    };
    let mut package = toml::map::Map::new();
    package.insert("name".to_string(), name.trim_start_matches("crates-io/").into());
    for key in ["version", "edition"] {
        if let Some(value) = if key == "edition" {
            facet.get(key)
        } else {
            project.get(key)
        } {
            package.insert(key.to_string(), value.clone());
        }
    }
    let mut library = toml::map::Map::new();
    library.insert("path".to_string(), source.into());
    if let Some(name) = facet.get("name") {
        library.insert("name".to_string(), name.clone());
    }
    library.insert(
        "proc-macro".to_string(),
        (facet.get("type").and_then(toml::Value::as_str) == Some("proc-macro")).into(),
    );
    let mut projection = toml::map::Map::new();
    projection.insert("package".to_string(), package.into());
    projection.insert("lib".to_string(), library.into());
    if let Some(features) = project.get("features") {
        projection.insert("features".to_string(), features.clone());
    }
    let mut dependencies = toml::map::Map::new();
    let mut targets = toml::map::Map::new();
    if let Some(table) = manifest.get("dependencies").and_then(toml::Value::as_table) {
        for (alias, values) in table {
            for value in values
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or(std::slice::from_ref(values))
            {
                let mut declaration = value
                    .as_table()
                    .cloned()
                    .ok_or_else(|| RustMetadataError::LoadWorkspace {
                        path: path.clone(),
                        message: format!("inspection dependency `{alias}` must be a Loaf declaration"),
                    })?;
                if let Some(loaf) = declaration
                    .remove("loaf")
                    .and_then(|value| value.as_str().map(str::to_string))
                {
                    declaration.insert("package".to_string(), loaf.trim_start_matches("crates-io/").into());
                }
                if let Some(target) = declaration
                    .remove("target")
                    .and_then(|value| value.as_str().map(str::to_string))
                {
                    let target = targets
                        .entry(target)
                        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
                    let table = target.as_table_mut().ok_or_else(|| RustMetadataError::LoadWorkspace {
                        path: path.clone(),
                        message: "invalid inspection target projection".to_string(),
                    })?;
                    let entries = table
                        .entry("dependencies".to_string())
                        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
                    let entries = entries.as_table_mut().ok_or_else(|| RustMetadataError::LoadWorkspace {
                        path: path.clone(),
                        message: "invalid inspection dependency projection".to_string(),
                    })?;
                    entries.insert(alias.clone(), declaration.into());
                } else {
                    dependencies.insert(alias.clone(), declaration.into());
                }
            }
        }
    }
    projection.insert("dependencies".to_string(), dependencies.into());
    projection.insert("target".to_string(), targets.into());
    Ok(projection.into())
}

#[cfg(test)]
mod tests {
    use crate::loader::{
        OvenInspectionRegistrySource, RustWorkspace, digest_oven_source_tree, write_oven_inspection_source_authority,
    };

    /// Adopted archives without Cargo metadata preserve their Rust facet and renamed dependency through inspection.
    #[test]
    fn loaf_sources_load_without_cargo_metadata() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let dependency = tempfile::tempdir()?;
        std::fs::create_dir_all(root.path().join("rust/src"))?;
        std::fs::create_dir_all(dependency.path().join("src"))?;
        std::fs::write(root.path().join("rust/src/lib.rs"), "pub use renamed::Value;")?;
        std::fs::write(dependency.path().join("src/lib.rs"), "pub struct Value;")?;
        std::fs::write(
            root.path().join("loaf.toml"),
            "[project]\nname='root'\nversion='1.0.0'\n[rust]\nname='root_facet'\ntype='lib'\nedition='2024'\n[rust.source]\nroot='rust'\n[dependencies]\nrenamed={loaf='crates-io/foreign',version='1',features=['selected'],default-features=false}\n",
        )?;
        std::fs::write(
            dependency.path().join("loaf.toml"),
            "[project]\nname='crates-io/foreign'\nversion='1.0.0'\n[project.features]\nselected=[]\n[rust]\nname='foreign'\ntype='lib'\nedition='2021'\n[rust.source]\nroot='src/lib.rs'\n",
        )?;
        write_oven_inspection_source_authority(
            root.path(),
            vec![OvenInspectionRegistrySource {
                package: "foreign".to_string(),
                version: "1.0.0".to_string(),
                registry: "registry+incan.pub/crates-io".to_string(),
                checksum: "sha256:fixture".to_string(),
                features: vec!["selected".to_string()],
                source_root: dependency.path().to_path_buf(),
                source_digest: digest_oven_source_tree(dependency.path())?,
            }],
        )?;
        let payload = RustWorkspace::oven_project_json_payload(root.path())?;
        let graph: serde_json::Value = serde_json::from_slice(&payload)?;
        assert_eq!(graph["crates"][0]["display_name"], "root_facet");
        assert_eq!(graph["crates"][0]["deps"][0]["name"], "renamed");
        assert_eq!(graph["crates"][1]["display_name"], "foreign");
        assert_eq!(graph["crates"][1]["cfg"][0], "feature=\"selected\"");
        assert!(!root.path().join("Cargo.toml").exists());
        assert!(!dependency.path().join("Cargo.toml").exists());
        let mut frozen = serde_json::json!({"crates": [graph["crates"][1].clone()]});
        frozen["crates"][0]["cfg"] = serde_json::json!(["selected_fact", "feature=\"selected\""]);
        let project = root.path().join(".incan_oven_loaf_project.json");
        std::fs::write(&project, serde_json::to_vec(&frozen)?)?;
        std::fs::write(root.path().join("Cargo.lock"), "invalid Cargo metadata")?;
        let loaded: serde_json::Value =
            serde_json::from_slice(&RustWorkspace::oven_project_json_payload(root.path())?)?;
        assert_eq!(loaded["crates"], frozen["crates"]);
        std::fs::write(root.path().join(super::super::OVEN_DIRECT_INSPECTION_MARKER), "")?;
        let workspace =
            RustWorkspace::load_with_options_and_target(root.path(), &root.path().join("inspection"), &|_| {}, false)?;
        assert!(workspace.crate_by_name("foreign").is_some());
        frozen["crates"][0]["deps"] = serde_json::json!([{"crate": 99, "name": "escape"}]);
        std::fs::write(&project, serde_json::to_vec(&frozen)?)?;
        assert!(RustWorkspace::oven_project_json_payload(root.path()).is_err());
        frozen["crates"][0]["deps"] = serde_json::json!([]);
        frozen["crates"][0]["root_module"] = serde_json::json!(root.path().join("rust/src/lib.rs"));
        std::fs::write(&project, serde_json::to_vec(&frozen)?)?;
        assert!(RustWorkspace::oven_project_json_payload(root.path()).is_err());
        Ok(())
    }
}
