//! Public namespace materialization over checked API metadata packages (issue 948).

use super::super::*;
use super::metadata_for;

#[test]
fn checked_api_materializes_public_namespace_members_and_children_issue948() -> Result<(), String> {
    let mut index = metadata_for("pub def build() -> int:\n  return 1\n").map_err(|errors| format!("{errors:?}"))?;
    index.module_path = vec!["main".to_string()];
    let mut root_only = CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: vec![index.clone()],
        public_namespaces: Vec::new(),
    };
    assert!(checked_api_public_module_paths(&root_only).is_empty());
    materialize_checked_api_public_namespaces(&mut root_only).map_err(|error| error.to_string())?;
    assert!(root_only.public_namespaces.is_empty());
    assert!(checked_api_public_module_paths(&root_only).is_empty());
    assert!(checked_api_public_namespace(&root_only, &["build".to_string()]).is_none());
    index.module_path = vec!["hyperquant".to_string(), "index".to_string()];
    let mut package = CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: vec![index],
        public_namespaces: Vec::new(),
    };

    let legacy = checked_api_public_namespace(&package, &["hyperquant".to_string()])
        .ok_or_else(|| "missing legacy parent namespace".to_string())?;
    assert!(checked_api_public_namespace(&package, &["missing".to_string()]).is_none());
    materialize_checked_api_public_namespaces(&mut package).map_err(|error| error.to_string())?;

    let parent = checked_api_public_namespace(&package, &["hyperquant".to_string()])
        .ok_or_else(|| "missing parent namespace".to_string())?;
    assert_eq!(parent, legacy);
    assert_eq!(parent.child_modules, vec!["index".to_string()]);
    assert_eq!(parent.members.len(), 1);
    assert_eq!(parent.members[0].name, "build");
    assert_eq!(
        parent.members[0].source_path,
        vec!["hyperquant".to_string(), "index".to_string(), "build".to_string()]
    );
    assert!(checked_api_public_namespace(&package, &["missing".to_string()]).is_none());
    assert!(
        checked_api_public_namespace(
            &package,
            &["hyperquant".to_string(), "index".to_string(), "build".to_string()]
        )
        .is_none()
    );
    Ok(())
}

#[test]
fn checked_api_rejects_duplicate_logical_source_modules_issue948() -> Result<(), String> {
    let mut first = metadata_for("pub def left() -> int:\n  return 1\n").map_err(|errors| format!("{errors:?}"))?;
    first.module_path = vec!["hyperquant".to_string()];
    let mut second = metadata_for("pub def right() -> int:\n  return 2\n").map_err(|errors| format!("{errors:?}"))?;
    second.module_path = vec!["hyperquant".to_string()];
    let mut package = CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: vec![first, second],
        public_namespaces: Vec::new(),
    };

    let Err(error) = materialize_checked_api_public_namespaces(&mut package) else {
        return Err("duplicate logical modules unexpectedly produced a public namespace graph".to_string());
    };

    assert!(error.to_string().contains("multiple source files"));
    assert!(error.to_string().contains("hyperquant"));
    Ok(())
}
