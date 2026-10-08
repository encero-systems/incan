//! Explicit, one-time Cargo declaration adoption. Native bakes never call this reader.

use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

use toml::{Value, map::Map};

use crate::{CliError, CliResult, ExitCode};

mod dependencies;

type Error = Box<dyn std::error::Error>;

/// Adopt the local dependency closure of selected workspace packages without invoking Cargo.
pub fn run(workspace: &Path, projects: &[PathBuf]) -> CliResult<ExitCode> {
    convert(workspace, projects).map_err(|error| CliError::failure(format!("convert-cargo: {error}")))?;
    Ok(ExitCode::SUCCESS)
}

/// Read explicit adoption inputs and validate every result before publishing any declaration.
fn convert(workspace: &Path, projects: &[PathBuf]) -> Result<(), Error> {
    let workspace = workspace.canonicalize()?;
    let root: Value = toml::from_str(&std::fs::read_to_string(workspace.join("Cargo.toml"))?)?;
    let lock: Value = toml::from_str(&std::fs::read_to_string(workspace.join("Cargo.lock"))?)?;
    let mut queue: VecDeque<_> = projects.iter().map(|path| workspace.join(path)).collect();
    let mut visited = BTreeSet::new();
    let mut outputs = Vec::new();
    while let Some(project) = queue.pop_front() {
        let project = project.canonicalize()?;
        if !project.starts_with(&workspace) {
            return Err(format!("adoption source {} escapes the workspace", project.display()).into());
        }
        if !visited.insert(project.clone()) {
            continue;
        }
        let cargo: Value = toml::from_str(&std::fs::read_to_string(project.join("Cargo.toml"))?)?;
        let mut loaf = declaration(&project, &cargo, &root)?;
        dependencies::convert(&project, &workspace, &cargo, &root, &lock, &mut loaf, &mut queue)?;
        let rendered = render(&loaf)?;
        oven_model::manifest::ProjectManifest::from_str(&rendered, &project.join("loaf.toml"))?;
        outputs.push((project.join("loaf.toml"), rendered));
    }
    for (path, rendered) in outputs {
        std::fs::write(&path, rendered)?;
        println!("converted {}", path.strip_prefix(&workspace)?.display());
    }
    Ok(())
}

/// Keep target alternatives inline so both TOML deserialization and the source-span admission gate see arrays.
fn render(loaf: &Value) -> Result<String, Error> {
    let mut document: toml_edit::DocumentMut = toml::to_string_pretty(loaf)?.parse()?;
    if let Some(dependencies) = loaf.get("dependencies").and_then(Value::as_table) {
        let mut table = toml_edit::Table::new();
        for (alias, declaration) in dependencies {
            table.insert(alias, toml_edit::Item::Value(inline(declaration)?));
        }
        document.insert("dependencies", toml_edit::Item::Table(table));
    }
    Ok(document.to_string())
}

/// Render dependency objects and arrays in the canonical authored dependency grammar without losing types.
fn inline(value: &Value) -> Result<toml_edit::Value, Error> {
    Ok(match value {
        Value::String(value) => toml_edit::Value::from(value.as_str()),
        Value::Boolean(value) => toml_edit::Value::from(*value),
        Value::Array(values) => {
            let mut array = toml_edit::Array::new();
            for value in values {
                array.push(inline(value)?);
            }
            toml_edit::Value::Array(array)
        }
        Value::Table(values) => {
            let mut table = toml_edit::InlineTable::new();
            for (key, value) in values {
                table.insert(key, inline(value)?);
            }
            toml_edit::Value::InlineTable(table)
        }
        _ => return Err("dependency declarations contain an unsupported value type".into()),
    })
}

/// Preserve existing Loaf-specific policy while replacing the selected Cargo library's authored facet.
fn declaration(project: &Path, cargo: &Value, workspace: &Value) -> Result<Value, Error> {
    let path = project.join("loaf.toml");
    let mut loaf = if path.is_file() {
        toml::from_str::<Value>(&std::fs::read_to_string(path)?)?
    } else {
        Value::Table(Map::new())
    };
    let package = cargo.get("package").ok_or("Cargo package table is missing")?;
    let mut metadata = loaf
        .get("project")
        .and_then(Value::as_table)
        .cloned()
        .unwrap_or_default();
    for key in ["name", "version", "description", "license"] {
        if let Some(value) = inherited(package, workspace, key)? {
            metadata.insert(key.to_string(), value);
        }
    }
    metadata.insert("private".to_string(), Value::Boolean(true));
    if let Some(features) = cargo.get("features") {
        metadata.insert("features".to_string(), features.clone());
    }
    loaf.as_table_mut()
        .ok_or("Loaf declaration is not a table")?
        .insert("project".to_string(), Value::Table(metadata));
    let lib = cargo.get("lib");
    let name = lib
        .and_then(|lib| lib.get("name"))
        .and_then(Value::as_str)
        .or_else(|| package.get("name").and_then(Value::as_str))
        .ok_or("package name is missing")?;
    let mut facet = loaf.get("rust").and_then(Value::as_table).cloned().unwrap_or_default();
    facet.insert("name".to_string(), Value::String(name.replace('-', "_")));
    facet.insert(
        "type".to_string(),
        Value::String(
            if lib.and_then(|lib| lib.get("proc-macro")).and_then(Value::as_bool) == Some(true) {
                "proc-macro"
            } else {
                "lib"
            }
            .to_string(),
        ),
    );
    facet.insert(
        "edition".to_string(),
        inherited(package, workspace, "edition")?.unwrap_or(Value::String("2015".to_string())),
    );
    if project.join("build.rs").is_file()
        || package
            .get("build")
            .is_some_and(|value| value != &Value::Boolean(false))
    {
        facet.insert("build-script".to_string(), Value::Boolean(true));
    }
    if lib
        .and_then(|lib| lib.get("path"))
        .and_then(Value::as_str)
        .is_some_and(|path| path != "src/lib.rs")
    {
        return Err("nonconventional library roots need an explicit source adaptation".into());
    }
    if !project.join("src/lib.rs").is_file() {
        return Err(format!("{} is not a conventional library package", project.display()).into());
    }
    loaf.as_table_mut()
        .ok_or("Loaf declaration is not a table")?
        .insert("rust".to_string(), Value::Table(facet));
    loaf.as_table_mut()
        .ok_or("Loaf declaration is not a table")?
        .remove("features");
    Ok(loaf)
}

/// Resolve package-field workspace inheritance only at the explicit conversion boundary.
fn inherited(package: &Value, root: &Value, key: &str) -> Result<Option<Value>, Error> {
    let Some(value) = package.get(key) else { return Ok(None) };
    if value.get("workspace").and_then(Value::as_bool) == Some(true) {
        return root
            .get("workspace")
            .and_then(|value| value.get("package"))
            .and_then(|value| value.get(key))
            .cloned()
            .map(Some)
            .ok_or_else(|| format!("workspace.package.{key} is missing").into());
    }
    Ok(Some(value.clone()))
}

#[cfg(test)]
mod tests {
    use super::convert;

    /// Adoption preserves workspace feature unions, renames, target alternatives, and lock-selected identities.
    #[test]
    fn converts_workspace_closure_without_running_cargo() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        std::fs::create_dir_all(root.path().join("a/src"))?;
        std::fs::create_dir_all(root.path().join("b/src"))?;
        std::fs::write(
            root.path().join("Cargo.toml"),
            "[workspace.package]\nversion='1.0.0'\nedition='2024'\n[workspace.dependencies]\nb={path='b',default-features=false}\nrenamed={package='serde',version='1',features=['derive']}\n",
        )?;
        std::fs::write(
            root.path().join("Cargo.lock"),
            "version=4\n[[package]]\nname='serde'\nversion='1.0.228'\nsource='registry+https://github.com/rust-lang/crates.io-index'\n",
        )?;
        std::fs::write(
            root.path().join("a/Cargo.toml"),
            "[package]\nname='a'\nversion.workspace=true\nedition.workspace=true\n[features]\noptional=['dep:renamed']\n[dependencies]\nb.workspace=true\nrenamed={workspace=true,features=['alloc'],optional=true}\n[target.'cfg(unix)'.dependencies]\nrenamed={package='serde',version='1',default-features=false}\n",
        )?;
        std::fs::write(
            root.path().join("b/Cargo.toml"),
            "[package]\nname='b'\nversion.workspace=true\nedition.workspace=true\n",
        )?;
        for member in ["a", "b"] {
            std::fs::write(root.path().join(member).join("src/lib.rs"), "")?;
        }
        convert(root.path(), &["a".into()])?;
        let loaf: toml::Value = toml::from_str(&std::fs::read_to_string(root.path().join("a/loaf.toml"))?)?;
        assert_eq!(loaf["project"]["version"].as_str(), Some("1.0.0"));
        assert_eq!(loaf["dependencies"]["b"]["path"].as_str(), Some("../b"));
        let alternatives = loaf["dependencies"]["renamed"]
            .as_array()
            .ok_or("missing alternatives")?;
        assert_eq!(alternatives.len(), 2);
        assert_eq!(alternatives[0]["loaf"].as_str(), Some("crates-io/serde"));
        assert_eq!(alternatives[0]["version"].as_str(), Some("1"));
        assert_eq!(alternatives[0]["features"].as_array().map(Vec::len), Some(2));
        assert!(root.path().join("b/loaf.toml").is_file());
        let first = std::fs::read(root.path().join("a/loaf.toml"))?;
        convert(root.path(), &["a".into()])?;
        assert_eq!(first, std::fs::read(root.path().join("a/loaf.toml"))?);
        Ok(())
    }

    /// An unsupported dependency prevents writes to the entire conversion closure.
    #[test]
    fn refuses_before_writing_any_manifest() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        std::fs::create_dir_all(root.path().join("a/src"))?;
        std::fs::write(root.path().join("Cargo.toml"), "[workspace]\nmembers=['a']\n")?;
        std::fs::write(root.path().join("Cargo.lock"), "version=4\npackage=[]\n")?;
        std::fs::write(
            root.path().join("a/Cargo.toml"),
            "[package]\nname='a'\nversion='1.0.0'\n[dependencies]\nunsupported={git='https://example.invalid/repository'}\n",
        )?;
        std::fs::write(root.path().join("a/src/lib.rs"), "")?;
        assert!(convert(root.path(), &["a".into()]).is_err());
        assert!(!root.path().join("a/loaf.toml").exists());
        Ok(())
    }
}
