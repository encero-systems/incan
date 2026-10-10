//! Actual admitted ordinary-session revalidation controls for explicit library preparation (#1337/#1698).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use incan_frontend::library_manifest_index::LibraryManifestIndex;
use incan_provider::{FeatureSelection, ProviderPlan};
use oven_store::store::OvenStoreLimits;

use super::current_admitted_library_session;
use crate::build::library_dependencies::PreparedLibraryDependencies;
use crate::session::CompilationSession;

/// Construct a real dependency-free ordinary session without selecting a native compiler or SDK.
fn session(
    root: &Path,
    features: &FeatureSelection,
) -> Result<(PathBuf, CompilationSession), Box<dyn std::error::Error>> {
    fs::create_dir_all(root.join("src"))?;
    fs::write(
        root.join("loaf.toml"),
        "[project]\nname='ordinary_input'\nversion='1.0.0'\n[project.features]\nextra=[]\n",
    )?;
    let entry = root.join("src/lib.incn");
    fs::write(&entry, "def value() -> int:\n    return 42\n")?;
    let dependencies = Arc::new(PreparedLibraryDependencies::admit(
        &[],
        "aarch64-apple-darwin",
        "ordinary-session-control",
        OvenStoreLimits::new(16 << 20, 16 << 20, 16 << 20),
    )?);
    let session = CompilationSession::discover_with_admitted_library_dependencies(&entry, features, dependencies)?;
    Ok((entry, session))
}

/// Revalidation preserves the genuine original dependency capability on first and unchanged repeat inputs.
#[test]
fn ordinary_explicit_preparation_reuses_original_dependency_capability() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let features = FeatureSelection::default();
    let (entry, input) = session(root.path(), &features)?;
    let original = input
        .admitted_library_dependencies()
        .ok_or("missing original dependencies")?;
    for _ in 0..2 {
        let current = current_admitted_library_session(&input, &entry, &features)?;
        assert!(Arc::ptr_eq(
            original,
            current
                .admitted_library_dependencies()
                .ok_or("missing retained dependencies")?,
        ));
        assert!(current.sdk_inventory.is_none());
        assert!(current.sdk_components.is_none());
        assert!(Arc::ptr_eq(&current.provider_plan, original.provider_plan()));
    }
    Ok(())
}

/// A genuine admission for another authored root cannot stand in for the selected package.
#[test]
fn ordinary_explicit_preparation_refuses_wrong_project() -> Result<(), Box<dyn std::error::Error>> {
    let first = tempfile::tempdir()?;
    let second = tempfile::tempdir()?;
    let features = FeatureSelection::default();
    let (_, input) = session(first.path(), &features)?;
    let (entry, _other) = session(second.path(), &features)?;
    let error = current_admitted_library_session(&input, &entry, &features)
        .err()
        .ok_or("wrong project admitted")?;
    assert!(error.to_string().contains("different project"), "{error}");
    Ok(())
}

/// Feature selection and the captured source graph must describe the same current ordinary package generation.
#[test]
fn ordinary_explicit_preparation_refuses_feature_drift() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let (entry, input) = session(root.path(), &FeatureSelection::default())?;
    let selected = FeatureSelection {
        requested: ["extra".to_string()].into(),
        ..FeatureSelection::default()
    };
    let error = current_admitted_library_session(&input, &entry, &selected)
        .err()
        .ok_or("changed features admitted")?;
    assert!(error.to_string().contains("features differ"), "{error}");
    let (_, selected_input) = session(root.path(), &selected)?;
    let current = current_admitted_library_session(&selected_input, &entry, &selected)?;
    assert_eq!(current.active_features, selected.requested);
    Ok(())
}

/// A public replacement provider plan cannot borrow the private ordinary dependency admission.
#[test]
fn ordinary_explicit_preparation_refuses_provider_substitution() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let features = FeatureSelection::default();
    let (entry, mut input) = session(root.path(), &features)?;
    input.provider_plan = Arc::new(ProviderPlan::new(
        LibraryManifestIndex::default(),
        Vec::new(),
        std::iter::empty(),
    )?);
    let error = current_admitted_library_session(&input, &entry, &features)
        .err()
        .ok_or("provider substitution admitted")?;
    assert!(error.to_string().contains("competing provider authority"), "{error}");
    Ok(())
}

/// Authored feature declarations changed since session capture cannot be hidden by the same selected feature set.
#[test]
fn ordinary_explicit_preparation_refuses_changed_feature_contract() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let features = FeatureSelection::default();
    let (entry, input) = session(root.path(), &features)?;
    fs::write(
        root.path().join("loaf.toml"),
        "[project]\nname='ordinary_input'\nversion='1.0.0'\n[project.features]\nextra=[]\nadded=[]\n",
    )?;
    let error = current_admitted_library_session(&input, &entry, &features)
        .err()
        .ok_or("changed feature contract admitted")?;
    assert!(error.to_string().contains("features differ"), "{error}");
    Ok(())
}
