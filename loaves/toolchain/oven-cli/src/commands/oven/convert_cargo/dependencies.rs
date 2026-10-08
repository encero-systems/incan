//! Dependency translation for the explicit Cargo adoption command.

use super::Error;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use toml::{Value, map::Map};

/// Translate ordinary and target-specific edges; build-only edges remain inert adoption inventory.
pub(super) fn convert(
    project: &Path,
    workspace: &Path,
    cargo: &Value,
    root: &Value,
    lock: &Value,
    loaf: &mut Value,
    queue: &mut VecDeque<PathBuf>,
) -> Result<(), Error> {
    let mut normal = Map::new();
    let mut build = Map::new();
    for (key, result) in [("dependencies", &mut normal), ("build-dependencies", &mut build)] {
        if let Some(table) = cargo.get(key).and_then(Value::as_table) {
            append(table, None, project, workspace, root, lock, result, queue)?;
        }
        if let Some(targets) = cargo.get("target").and_then(Value::as_table) {
            for (target, table) in targets {
                if let Some(table) = table.get(key).and_then(Value::as_table) {
                    append(table, Some(target), project, workspace, root, lock, result, queue)?;
                }
            }
        }
    }
    loaf.as_table_mut()
        .ok_or("Loaf declaration is not a table")?
        .insert("dependencies".to_string(), Value::Table(normal));
    if !build.is_empty() {
        let table = loaf.as_table_mut().ok_or("Loaf declaration is not a table")?;
        let tool = table
            .entry("tool".to_string())
            .or_insert_with(|| Value::Table(Map::new()));
        tool.as_table_mut().ok_or("tool must be a table")?.insert(
            "cargo-adoption".to_string(),
            Value::Table(Map::from_iter([(
                "build-dependencies".to_string(),
                Value::Table(build),
            )])),
        );
    }
    Ok(())
}

/// Preserve target alternatives as separate declarations rather than merging conditional requirements.
#[allow(clippy::too_many_arguments)]
fn append(
    table: &Map<String, Value>,
    target: Option<&str>,
    project: &Path,
    workspace: &Path,
    root: &Value,
    lock: &Value,
    result: &mut Map<String, Value>,
    queue: &mut VecDeque<PathBuf>,
) -> Result<(), Error> {
    for (alias, value) in table {
        let mut declaration = dependency(alias, value, project, workspace, root, lock, queue)?;
        if let Some(target) = target {
            declaration
                .as_table_mut()
                .ok_or("dependency declaration must be a table")?
                .insert("target".to_string(), Value::String(target.to_string()));
        }
        match result.remove(alias) {
            None => {
                result.insert(alias.clone(), declaration);
            }
            Some(Value::Array(mut entries)) => {
                entries.push(declaration);
                result.insert(alias.clone(), Value::Array(entries));
            }
            Some(mut existing) => {
                existing
                    .as_table_mut()
                    .ok_or("dependency declaration must be a table")?
                    .entry("target".to_string())
                    .or_insert_with(|| Value::String("cfg(all())".to_string()));
                result.insert(alias.clone(), Value::Array(vec![existing, declaration]));
            }
        }
    }
    Ok(())
}

/// Flatten Cargo inheritance and retain registry requirements after checking the supplied adoption lock.
fn dependency(
    alias: &str,
    value: &Value,
    project: &Path,
    workspace: &Path,
    root: &Value,
    lock: &Value,
    queue: &mut VecDeque<PathBuf>,
) -> Result<Value, Error> {
    let mut entry = match value {
        Value::String(version) => Map::from_iter([("version".to_string(), Value::String(version.clone()))]),
        Value::Table(table) => table.clone(),
        _ => return Err(format!("dependency {alias} must be a string or table").into()),
    };
    let inherited = entry.remove("workspace").and_then(|value| value.as_bool()) == Some(true);
    if inherited {
        let shared = root
            .get("workspace")
            .and_then(|value| value.get("dependencies"))
            .and_then(|value| value.get(alias))
            .ok_or_else(|| format!("workspace dependency {alias} is missing"))?;
        let shared = match shared {
            Value::String(version) => Map::from_iter([("version".to_string(), Value::String(version.clone()))]),
            Value::Table(table) => table.clone(),
            _ => return Err("workspace dependency must be a string or table".into()),
        };
        let extra_features = entry.remove("features");
        for (key, value) in shared {
            entry.entry(key).or_insert(value);
        }
        if let Some(Value::Array(features)) = extra_features {
            let selected = entry
                .entry("features".to_string())
                .or_insert_with(|| Value::Array(Vec::new()));
            selected
                .as_array_mut()
                .ok_or("dependency features must be an array")?
                .extend(features);
        }
    }
    if entry.contains_key("git") || entry.contains_key("registry") {
        return Err(format!("dependency {alias} needs an explicit registered Loaf adaptation").into());
    }
    let package = entry
        .remove("package")
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| alias.to_string());
    if let Some(Value::String(path)) = entry.remove("path") {
        let owner = if inherited { workspace } else { project };
        let path = owner.join(path).canonicalize()?;
        let manifest: Value = toml::from_str(&std::fs::read_to_string(path.join("Cargo.toml"))?)?;
        let name = manifest
            .get("package")
            .and_then(|value| value.get("name"))
            .and_then(Value::as_str)
            .ok_or("path package name is missing")?;
        entry.insert("loaf".to_string(), Value::String(name.to_string()));
        entry.insert("path".to_string(), Value::String(relative(project, &path)?));
        queue.push_back(path);
    } else {
        let requirement = entry
            .get("version")
            .and_then(Value::as_str)
            .ok_or("registry dependency needs a version")?;
        let requirement = semver::VersionReq::parse(requirement)?;
        let versions = lock
            .get("package")
            .and_then(Value::as_array)
            .ok_or("Cargo lock packages are missing")?
            .iter()
            .filter(|value| value.get("name").and_then(Value::as_str) == Some(&package))
            .filter(|value| {
                value
                    .get("source")
                    .and_then(Value::as_str)
                    .is_some_and(|source| source.starts_with("registry+"))
            })
            .filter_map(|value| value.get("version").and_then(Value::as_str))
            .map(semver::Version::parse)
            .collect::<Result<Vec<_>, _>>()?;
        let matching: Vec<_> = versions
            .into_iter()
            .filter(|version| requirement.matches(version))
            .collect();
        let [_version] = matching.as_slice() else {
            return Err(format!("{package}: lock must select exactly one version matching {requirement}").into());
        };
        entry.insert("loaf".to_string(), Value::String(format!("crates-io/{package}")));
    }
    Ok(Value::Table(entry))
}

/// Emit a portable owner-relative path after canonicalizing both conversion inputs.
fn relative(owner: &Path, destination: &Path) -> Result<String, Error> {
    let owner: Vec<_> = owner.components().collect();
    let destination: Vec<_> = destination.components().collect();
    let common = owner
        .iter()
        .zip(&destination)
        .take_while(|(left, right)| left == right)
        .count();
    let mut path = PathBuf::new();
    for _ in common..owner.len() {
        path.push("..");
    }
    for component in &destination[common..] {
        path.push(component.as_os_str());
    }
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| "dependency path is not UTF-8".into())
}
