use std::path::Path;
use std::sync::OnceLock;

/// Parse one component-entrypoint module import into the provider path it exposes.
///
/// An import alias changes only the local binding. Provider authority remains attached to the imported module path,
/// and prelude imports continue to expose their parent facade.
fn imported_provider_path(line: &str) -> Option<Vec<String>> {
    let imported_module = line.strip_prefix("import ")?;
    let imported_module = imported_module
        .split_once(" as ")
        .map_or(imported_module, |(module_path, _)| module_path);
    let mut segments = imported_module.split('.').map(str::to_owned).collect::<Vec<_>>();
    if segments.len() > 1 && segments.last().map(String::as_str) == Some("prelude") {
        segments.pop();
    }
    Some(segments)
}

/// Derive codegen fixture ownership from the same SDK component catalog and Incan entrypoints used by publication.
pub(crate) fn artifact_module_paths() -> Vec<Vec<String>> {
    static MODULE_PATHS: OnceLock<Vec<Vec<String>>> = OnceLock::new();
    MODULE_PATHS
        .get_or_init(|| {
            let stdlib_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/incan_stdlib/stdlib");
            let catalog_path = stdlib_root.join("sdk-components.toml");
            let Ok(catalog_source) = std::fs::read_to_string(&catalog_path) else {
                panic!("failed to read SDK component catalog at {}", catalog_path.display());
            };
            let Ok(catalog) = toml::from_str::<toml::Value>(&catalog_source) else {
                panic!("failed to parse SDK component catalog at {}", catalog_path.display());
            };
            let Some(components) = catalog.get("components").and_then(toml::Value::as_table) else {
                panic!(
                    "SDK component catalog at {} has no component table",
                    catalog_path.display()
                );
            };
            let mut paths = Vec::new();
            for component in components.values() {
                let Some(project) = component.get("project").and_then(toml::Value::as_str) else {
                    panic!(
                        "SDK component catalog at {} has a component without a project",
                        catalog_path.display()
                    );
                };
                let entrypoint = stdlib_root.join(project).join("src/lib.incn");
                let Ok(source) = std::fs::read_to_string(&entrypoint) else {
                    panic!("failed to read SDK component entrypoint at {}", entrypoint.display());
                };
                paths.extend(source.lines().filter_map(imported_provider_path));
            }
            paths.sort();
            paths.dedup();
            paths
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::imported_provider_path;

    #[test]
    fn provider_path_ignores_local_import_aliases() {
        assert_eq!(
            imported_provider_path("import web.app as web_app"),
            Some(vec!["web".to_string(), "app".to_string()])
        );
        assert_eq!(
            imported_provider_path("import async.prelude as async_prelude"),
            Some(vec!["async".to_string()])
        );
    }
}
