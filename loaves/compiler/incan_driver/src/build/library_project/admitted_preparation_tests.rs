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

/// Checking retains inferred receiver demand and complete publication extracts it without an explicit type import.
#[cfg(feature = "rust_inspect")]
#[test]
fn checked_library_abi_retains_inferred_rust_receiver() -> Result<(), Box<dyn std::error::Error>> {
    // ---- Ordinary source session and real foreign factory ----
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    fs::write(
        root.path().join("loaf.toml"),
        "[project]\nname='ordinary_input'\nversion='1.0.0'\n[rust-dependencies]\nabi_demand_probe='1.0.0'\n",
    )?;
    let entry = root.path().join("src/lib.incn");
    fs::write(
        &entry,
        "from rust::abi_demand_probe import make\n\npub def ready() -> bool:\n    return make().ready()\n",
    )?;
    let dependencies = Arc::new(PreparedLibraryDependencies::admit(
        &[],
        "aarch64-apple-darwin",
        "ordinary-session-control",
        OvenStoreLimits::new(16 << 20, 16 << 20, 16 << 20),
    )?);
    let input = CompilationSession::discover_with_admitted_library_dependencies(
        &entry,
        &FeatureSelection::default(),
        dependencies,
    )?;
    let probe = tempfile::tempdir()?;
    fs::create_dir(probe.path().join("src"))?;
    fs::write(
        probe.path().join("loaf.toml"),
        "[project]\nname='abi_demand_probe'\nversion='1.0.0'\n[rust]\nname='abi_demand_probe'\ntype='lib'\nedition='2021'\n",
    )?;
    fs::write(
        probe.path().join("src/lib.rs"),
        "pub struct Record;\nimpl Record { pub fn ready(&self) -> bool { true } }\npub fn make() -> Record { Record }\npub struct Unused;\n",
    )?;
    crate::lock::registry_sources::acquire_explicit_project_inspection_sources(probe.path(), probe.path(), &[])?;
    fs::write(probe.path().join(rust_inspect::OVEN_DIRECT_INSPECTION_MARKER), "1\n")?;
    fs::write(probe.path().join(rust_inspect::OVEN_LOAF_ONLY_INSPECTION_MARKER), "1\n")?;
    let modules = crate::modules::collect_library_modules_detailed_with_session(entry, &input)
        .map_err(|failure| failure.render_human())?;
    let plan = input.provider_plan_for_modules(&modules)?;
    let metadata = super::checked_public_library_metadata(
        input.manifest.as_ref().ok_or("project manifest missing")?,
        &input,
        &modules,
        &plan,
        "ordinary_input",
        "1.0.0",
        &mut std::collections::BTreeMap::new(),
        Some(probe.path()),
    )?;
    let initial = crate::build::library_exports::collect_library_rust_abi_query_paths(&modules, &[]);
    assert_eq!(initial, ["abi_demand_probe::make"]);
    assert!(metadata.rust_metadata_queries.contains("abi_demand_probe::Record"));
    assert!(!metadata.rust_metadata_queries.contains("abi_demand_probe::Unused"));
    let paths = initial
        .into_iter()
        .chain(metadata.rust_metadata_queries)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let abi = crate::build::library_exports::collect_library_rust_abi(probe.path(), &paths)?
        .ok_or("published ABI missing")?;
    let incan_lang::interop::RustItemKind::Type(record) = &abi
        .get("abi_demand_probe::Record")
        .ok_or("inferred receiver ABI missing")?
        .kind
    else {
        return Err("inferred receiver is not a type".into());
    };
    assert!(record.metadata_completeness.has_trait_impls());
    assert!(record.methods.iter().any(|method| method.name == "ready"));
    assert!(abi.get("abi_demand_probe::Unused").is_none());

    // ---- Complete shipped ABI needs no source inspection workspace ----
    let mut published = incan_frontend::library_manifest::LibraryManifest::new("runtime_facade", "1.0.0");
    published.rust_abi = Some(abi.clone());
    let index = LibraryManifestIndex::from_entries(std::collections::HashMap::from([(
        "runtime_facade".to_string(),
        incan_frontend::library_manifest_index::LibraryManifestIndexEntry::Loaded {
            manifest: Box::new(published.clone()),
            metadata: incan_frontend::library_manifest_index::LibraryArtifactMetadata::from_crate_root(
                "runtime_facade",
                "runtime_facade",
                root.path().join("published"),
            ),
        },
    )]));
    let unprepared = tempfile::tempdir()?;
    assert!(crate::build::library_exports::library_rust_abi_source_queries(&paths, &index).is_empty());
    fs::write(
        unprepared.path().join(rust_inspect::OVEN_LOAF_ONLY_INSPECTION_MARKER),
        "1\n",
    )?;
    for workspace in [None, Some(unprepared.path())] {
        assert_eq!(
            crate::build::library_exports::collect_library_rust_abi_with_shipped(workspace, &paths, &index)?,
            Some(abi.clone()),
            "complete shipped ABI must not need a new source inspection"
        );
    }
    // ---- Partial shipped records cannot replace complete extraction ----
    let partial = published
        .rust_abi
        .as_mut()
        .ok_or("published ABI missing")?
        .items
        .iter_mut()
        .find(|item| item.canonical_path == "abi_demand_probe::Record")
        .ok_or("receiver missing")?;
    let incan_lang::interop::RustItemKind::Type(partial) = &mut partial.kind else {
        return Err("receiver is not a type".into());
    };
    partial.metadata_completeness = incan_lang::interop::RustTypeMetadataCompleteness::FieldsAndVariantsOnly;
    let incomplete = LibraryManifestIndex::from_entries(std::collections::HashMap::from([(
        "runtime_facade".to_string(),
        incan_frontend::library_manifest_index::LibraryManifestIndexEntry::Loaded {
            manifest: Box::new(published),
            metadata: incan_frontend::library_manifest_index::LibraryArtifactMetadata::from_crate_root(
                "runtime_facade",
                "runtime_facade",
                root.path().join("partial"),
            ),
        },
    )]));
    let receiver = vec!["abi_demand_probe::Record".to_string()];
    assert_eq!(
        crate::build::library_exports::library_rust_abi_source_queries(&receiver, &incomplete),
        receiver
    );
    assert!(
        crate::build::library_exports::collect_library_rust_abi_with_shipped(None, &receiver, &incomplete)?.is_none()
    );
    assert!(
        crate::build::library_exports::collect_library_rust_abi_with_shipped(
            Some(unprepared.path()),
            &receiver,
            &incomplete
        )
        .is_err()
    );
    assert!(input.sdk_inventory.is_none());
    Ok(())
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
