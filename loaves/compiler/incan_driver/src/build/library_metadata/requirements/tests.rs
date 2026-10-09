//! Actual checked planning capture and current-binding replay controls (#1337/#1698).

use super::{CheckedLibraryCapture, CheckedLibraryRequirements};
use incan_frontend::library_exports::collect_checked_public_exports;
use incan_frontend::library_manifest_index::LibraryManifestIndex;
use incan_frontend::typechecker::TypeChecker;
use incan_provider::requirements::ProjectRequirements;
use oven_model::manifest::ProjectManifest;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;

/// Use frontend-checked declarations and the current ordinary project manifest as capture authority.
fn contract(name: &str, root: &std::path::Path) -> Result<CheckedLibraryRequirements, Box<dyn std::error::Error>> {
    fs::write(
        root.join("loaf.toml"),
        format!("[project]\nname={name:?}\nversion='1.0.0'\n"),
    )?;
    let source = "pub def scale(mut value: int, factor: int = 3) -> int:\n    return value * factor\n";
    let tokens = incan_frontend::lexer::lex(source).map_err(|errors| format!("lex: {errors:?}"))?;
    let program = incan_frontend::parser::parse(&tokens).map_err(|errors| format!("parse: {errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker.set_current_package_identity(Some(name.to_string()));
    checker.set_current_module_path(Some(vec!["lib".to_string()]));
    checker
        .check_program(&program)
        .map_err(|errors| format!("check: {errors:?}"))?;
    let exports = collect_checked_public_exports(&program, &checker);
    let manifest = ProjectManifest::load(&root.join("loaf.toml"))?;
    Ok(CheckedLibraryRequirements::capture(CheckedLibraryCapture {
        project: &manifest,
        index: &LibraryManifestIndex::default(),
        requirements: &ProjectRequirements::default(),
        imports: &[],
        exports: &exports,
        version: "1.0.0",
        used_module_paths: BTreeSet::from([vec!["std".to_string(), "core".to_string()]]),
        source_modules: BTreeMap::from([("src/lib.incn".to_string(), vec!["lib".to_string()])]),
        entry_module: vec!["lib".to_string()],
        rust_abi_queries: BTreeSet::new(),
        rust_extern_paths: vec!["companion::scale".to_string()],
        backend: None,
    })?)
}

/// Ordinary and standard names round-trip the same checked default, mutability, identity and planning contract.
#[test]
fn ordinary_checked_requirements_roundtrip_preserves_exports_and_current_bindings()
-> Result<(), Box<dyn std::error::Error>> {
    for name in ["ordinary_geometry", "incan_std_core"] {
        let root = tempfile::tempdir()?;
        let original = contract(name, root.path())?;
        let bytes = serde_json::to_vec(&original)?;
        let decoded: CheckedLibraryRequirements = serde_json::from_slice(&bytes)?;
        decoded.validate()?;
        let exports = decoded.caller_exports()?;
        assert_eq!(exports.len(), 1);
        assert_eq!(exports[0].identity.source_path, vec!["scale"]);
        assert_eq!(
            exports[0].identity.canonical,
            original.checked_exports[0].to_checked()?.identity.canonical
        );
        assert_eq!(decoded.rust_extern_paths, vec!["companion::scale"]);
        assert_eq!(decoded.used_module_paths, original.used_module_paths);
        let manifest = ProjectManifest::load(&root.path().join("loaf.toml"))?;
        let requirements = decoded.current_requirements(&manifest, &LibraryManifestIndex::default())?;
        let (resolved, imports) = decoded.current_dependencies(&manifest, &requirements)?;
        assert!(resolved.dependencies.is_empty());
        assert!(imports.is_empty());
    }
    Ok(())
}

/// Missing planning knowledge, malformed provenance and incomplete checked export authority cannot become hits.
#[test]
fn ordinary_checked_requirements_refuses_missing_or_corrupt_planning_authority()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let original = contract("ordinary", root.path())?;
    let mut version = original.clone();
    version.schema_version += 1;
    assert!(version.validate().is_err());
    let mut missing = original.clone();
    missing.source_modules.clear();
    assert!(missing.validate().is_err());
    let mut traversal = original.clone();
    traversal.source_modules = BTreeMap::from([("../escaped.incn".to_string(), vec!["lib".to_string()])]);
    assert!(traversal.validate().is_err());
    let mut payload: serde_json::Value = serde_json::to_value(&original)?;
    payload["checked_exports"][0]["schema_version"] = serde_json::json!(999);
    let corrupted: CheckedLibraryRequirements = serde_json::from_value(payload)?;
    assert!(corrupted.validate().is_err());
    Ok(())
}
