//! Resolve selected Cargo workspace declarations before direct path-library planning.
//!
//! This is manifest projection, never a resolver or a Cargo invocation. Feature activation and registry admission
//! remain the path planner's responsibility after inheritance has been made explicit.

use std::path::Path;

use super::{OvenRustcError, fs};

/// Expand inherited package fields and unconditional dependencies against their declaring workspace.
///
/// Only selected declarations enter the projection. Workspace dependency paths are anchored at the workspace root,
/// not at the member. Missing authority and illegal member overrides fail before compilation.
pub fn effective_path_manifest(path: &Path, mut manifest: toml::Value) -> Result<toml::Value, OvenRustcError> {
    let root = path.parent().ok_or_else(|| invalid("manifest has no parent"))?;
    let explicit = manifest.get("package").and_then(|p| p.get("workspace"));
    let workspace_path = if let Some(explicit) = explicit {
        Some(
            root.join(
                explicit
                    .as_str()
                    .ok_or_else(|| invalid("package.workspace must be a path"))?,
            )
            .join("Cargo.toml"),
        )
    } else if manifest.get("workspace").is_some() {
        Some(path.to_path_buf())
    } else {
        containing_workspace(root)?
    };
    let Some(workspace_path) = workspace_path else {
        for section in ["package", "dependencies"] {
            if let Some(table) = manifest.get(section).and_then(toml::Value::as_table) {
                for value in table.values() {
                    if inherits(value)? {
                        return Err(invalid("inherited declaration has no containing Cargo workspace"));
                    }
                }
            }
        }
        return Ok(manifest);
    };
    let text = fs::read_to_string(&workspace_path).map_err(|source| OvenRustcError::Io {
        path: workspace_path.clone(),
        source,
    })?;
    let workspace = toml::from_str::<toml::Value>(&text)
        .map_err(|error| invalid(format!("{}: {error}", workspace_path.display())))?;
    let workspace = workspace
        .get("workspace")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| invalid(format!("{} has no workspace table", workspace_path.display())))?;
    if let Some(package) = manifest.get_mut("package").and_then(toml::Value::as_table_mut) {
        for (name, value) in package {
            if !inherits(value)? {
                continue;
            }
            *value = workspace
                .get("package")
                .and_then(|p| p.get(name.as_str()))
                .cloned()
                .ok_or_else(|| invalid(format!("{} has no workspace.package.{name}", workspace_path.display())))?;
        }
    }
    if let Some(dependencies) = manifest.get_mut("dependencies").and_then(toml::Value::as_table_mut) {
        for (name, value) in dependencies {
            if !inherits(value)? {
                continue;
            }
            let member = value
                .as_table()
                .ok_or_else(|| invalid("inherited dependency is not a table"))?;
            let inherited = workspace
                .get("dependencies")
                .and_then(|d| d.get(name.as_str()))
                .ok_or_else(|| {
                    invalid(format!(
                        "{} has no workspace.dependencies.{name}",
                        workspace_path.display()
                    ))
                })?;
            let mut effective = merge_dependency(name, inherited, member)?;
            if let Some(dependency_path) = effective.get_mut("path") {
                let dependency_root = workspace_path
                    .parent()
                    .ok_or_else(|| invalid("workspace has no parent"))?;
                *dependency_path = toml::Value::String(
                    dependency_root
                        .join(
                            dependency_path
                                .as_str()
                                .ok_or_else(|| invalid("workspace dependency path must be a string"))?,
                        )
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            *value = toml::Value::Table(effective);
        }
    }
    Ok(manifest)
}

/// Discover ancestor authority while preserving malformed-manifest and I/O failures.
fn containing_workspace(root: &Path) -> Result<Option<std::path::PathBuf>, OvenRustcError> {
    for ancestor in root.ancestors().skip(1) {
        let candidate = ancestor.join("Cargo.toml");
        let text = match fs::read_to_string(&candidate) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(OvenRustcError::Io {
                    path: candidate,
                    source,
                });
            }
        };
        let manifest = toml::from_str::<toml::Value>(&text)
            .map_err(|error| invalid(format!("{}: {error}", candidate.display())))?;
        if manifest.get("workspace").is_some() {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

/// Recognize inheritance without accepting false or malformed workspace selectors.
fn inherits(value: &toml::Value) -> Result<bool, OvenRustcError> {
    match value.as_table().and_then(|table| table.get("workspace")) {
        None => Ok(false),
        Some(value) if value.as_bool() == Some(true) => Ok(true),
        Some(_) => Err(invalid("workspace inheritance must set workspace = true")),
    }
}

/// Union selected feature names and retain only Cargo's permitted member modifiers.
fn merge_dependency(name: &str, inherited: &toml::Value, member: &toml::Table) -> Result<toml::Table, OvenRustcError> {
    let mut effective = match inherited {
        toml::Value::String(version) => {
            toml::Table::from_iter([("version".into(), toml::Value::String(version.clone()))])
        }
        toml::Value::Table(table) if !table.contains_key("workspace") => table.clone(),
        _ => {
            return Err(invalid(format!(
                "workspace dependency `{name}` has an unsupported shape"
            )));
        }
    };
    for (key, value) in member {
        match key.as_str() {
            "workspace" => {}
            "features" => {
                let mut features = std::collections::BTreeSet::new();
                for selection in effective.get("features").into_iter().chain(Some(value)) {
                    for feature in selection
                        .as_array()
                        .ok_or_else(|| invalid("features must be an array"))?
                    {
                        features.insert(
                            feature
                                .as_str()
                                .ok_or_else(|| invalid("feature must be a string"))?
                                .to_string(),
                        );
                    }
                }
                effective.insert(
                    key.clone(),
                    toml::Value::Array(features.into_iter().map(toml::Value::String).collect()),
                );
            }
            "optional" | "default-features" | "public" if value.is_bool() => {
                effective.insert(key.clone(), value.clone());
            }
            _ => {
                return Err(invalid(format!(
                    "dependency `{name}` cannot override workspace field `{key}`"
                )));
            }
        }
    }
    Ok(effective)
}

/// Keep projection failures in the same typed category as unsupported direct path manifests.
fn invalid(message: impl Into<String>) -> OvenRustcError {
    OvenRustcError::InvalidInput {
        field: "path Rust dependency workspace",
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::effective_path_manifest;
    use std::fs;

    /// Inherited paths use the root declaration and unselected root fields do not affect the projection.
    #[test]
    fn projects_selected_workspace_authority() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let member = root.path().join("members/member");
        fs::create_dir_all(&member)?;
        let workspace = "[workspace]\n[workspace.package]\nedition = '2024'\nlicense = 'MIT'\n[workspace.dependencies]\nhelper = { path = 'helper', features = ['root'] }\n";
        fs::write(root.path().join("Cargo.toml"), workspace)?;
        let manifest: toml::Value = toml::from_str(
            "[package]\nname = 'member'\nedition.workspace = true\n[dependencies]\nhelper = { workspace = true, features = ['member'] }\n",
        )?;
        let path = member.join("Cargo.toml");
        let effective = effective_path_manifest(&path, manifest.clone())?;
        assert_eq!(effective["package"]["edition"].as_str(), Some("2024"));
        assert_eq!(
            effective["dependencies"]["helper"]["path"].as_str(),
            root.path().join("helper").to_str()
        );
        assert_eq!(
            effective["dependencies"]["helper"]["features"].as_array().map(Vec::len),
            Some(2)
        );
        fs::write(root.path().join("Cargo.toml"), workspace.replace("MIT", "Apache-2.0"))?;
        assert_eq!(effective, effective_path_manifest(&path, manifest.clone())?);
        fs::write(root.path().join("Cargo.toml"), workspace.replace("2024", "2021"))?;
        assert_ne!(effective, effective_path_manifest(&path, manifest)?);
        Ok(())
    }

    /// Missing inherited keys and member-local source overrides cannot create new authority.
    #[test]
    fn refuses_missing_and_overridden_workspace_authority() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let member = root.path().join("member");
        fs::create_dir_all(&member)?;
        fs::write(
            root.path().join("Cargo.toml"),
            "[workspace]\n[workspace.dependencies]\nhelper = '1'\n",
        )?;
        for dependency in [
            "helper = { workspace = true, path = 'other' }",
            "missing = { workspace = true }",
        ] {
            let manifest: toml::Value =
                toml::from_str(&format!("[package]\nname = 'member'\n[dependencies]\n{dependency}\n"))?;
            assert!(effective_path_manifest(&member.join("Cargo.toml"), manifest).is_err());
        }
        Ok(())
    }
}
