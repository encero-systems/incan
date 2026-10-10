//! Genuine canonical lock-writer controls with original ordinary session inputs (#1337/#1698).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use incan_provider::FeatureSelection;
use incan_provider::test_support::parsed_module_for_test;
use oven_model::lock::{IncanLock, LOCK_FILENAME};
use oven_store::store::OvenStoreLimits;

use super::publish_oven_project_lock_with_admitted_session;
use crate::build::library_dependencies::PreparedLibraryDependencies;
use crate::lock::{INERT_CARGO_LOCK_PAYLOAD, project_lock_collection_counts, reset_project_lock_collection_metrics};
use crate::session::CompilationSession;

/// Build a genuine dependency-free ordinary session; no native compiler admission is necessary to write its lock.
fn ordinary_session(root: &Path) -> Result<(PathBuf, CompilationSession), Box<dyn std::error::Error>> {
    fs::create_dir_all(root.join("src"))?;
    fs::write(
        root.join("loaf.toml"),
        "[project]\nname='ordinary_lock'\nversion='1.0.0'\n",
    )?;
    let entry = root.join("src/lib.incn");
    fs::write(&entry, "def value() -> int:\n    return 42\n")?;
    let dependencies = Arc::new(PreparedLibraryDependencies::admit(
        &[],
        "aarch64-apple-darwin",
        "ordinary-lock-control",
        OvenStoreLimits::new(16 << 20, 16 << 20, 16 << 20),
    )?);
    let session = CompilationSession::discover_with_admitted_library_dependencies(
        &entry,
        &FeatureSelection::default(),
        dependencies,
    )?;
    Ok((entry, session))
}

/// First publication and unchanged repeat use the actual writer and never discover a collector session.
#[test]
fn admitted_ordinary_lock_writer_retains_session_and_exact_proof() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let (entry, session) = ordinary_session(root.path())?;
    let canonical_root = fs::canonicalize(root.path())?;
    let mut digest = None;
    for _ in 0..2 {
        reset_project_lock_collection_metrics();
        let published = publish_oven_project_lock_with_admitted_session(
            &canonical_root,
            &entry,
            &FeatureSelection::default(),
            &session,
        )?;
        published.verify_published_file()?;
        assert_eq!(project_lock_collection_counts(), (1, 0));
        let path = canonical_root.join(LOCK_FILENAME);
        assert_eq!(published.canonical_lock_path(), path);
        let bytes = fs::read(&path)?;
        assert_eq!(published.published_content_digest(), oven_store::digest_bytes(&bytes));
        let lock = IncanLock::load(&path)?;
        assert_eq!(lock.cargo_lock_payload, INERT_CARGO_LOCK_PAYLOAD);
        if let Some(previous) = &digest {
            assert_eq!(previous, published.published_content_digest());
        }
        digest = Some(published.published_content_digest().to_string());
    }
    Ok(())
}

/// A genuine session for one package cannot publish another package's lock.
#[test]
fn admitted_ordinary_lock_writer_refuses_wrong_root() -> Result<(), Box<dyn std::error::Error>> {
    let first = tempfile::tempdir()?;
    let second = tempfile::tempdir()?;
    let (_, input) = ordinary_session(first.path())?;
    let (entry, _) = ordinary_session(second.path())?;
    reset_project_lock_collection_metrics();
    let error =
        publish_oven_project_lock_with_admitted_session(second.path(), &entry, &FeatureSelection::default(), &input)
            .err()
            .ok_or("wrong-root lock was published")?;
    assert!(error.to_string().contains("different project"), "{error}");
    assert_eq!(project_lock_collection_counts(), (0, 0));
    assert!(!second.path().join(LOCK_FILENAME).exists());
    Ok(())
}

/// A single admitted package cannot authorize workspace siblings through hidden session discovery.
#[test]
fn admitted_ordinary_lock_writer_refuses_workspace_sibling_authority() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join("loaf.toml"), "[workspace]\nmembers=['member']\n")?;
    let member = root.path().join("member");
    let (entry, input) = ordinary_session(&member)?;
    reset_project_lock_collection_metrics();
    let error = publish_oven_project_lock_with_admitted_session(&member, &entry, &FeatureSelection::default(), &input)
        .err()
        .ok_or("workspace lock published with only member authority")?;
    assert!(error.to_string().contains("standalone project authority"), "{error}");
    assert_eq!(project_lock_collection_counts(), (0, 0));
    assert!(!root.path().join(LOCK_FILENAME).exists());
    Ok(())
}

/// A genuine admitted session accepts source-only scalar usage and refuses every explicit root-prelude spelling.
#[test]
fn admitted_ordinary_module_usage_refuses_explicit_prelude_without_owner() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let (_, session) = ordinary_session(root.path())?;
    let scalar = parsed_module_for_test("pub def answer() -> int:\n    return 42\n")?;
    assert!(session.provider_module_paths(std::slice::from_ref(&scalar)).is_empty());
    let first = session.provider_plan_for_modules(std::slice::from_ref(&scalar))?;
    let repeat = session.provider_plan_for_modules(&[scalar])?;
    assert!(Arc::ptr_eq(&first, &repeat));
    for source in [
        "import std.prelude\n",
        "from std.prelude import Clone\n",
        "from std import prelude\n",
    ] {
        let parsed = parsed_module_for_test(source)?;
        let paths = session.provider_module_paths(std::slice::from_ref(&parsed));
        assert!(paths.contains(&vec!["std".to_string(), "prelude".to_string()]));
        let error = session
            .provider_plan_for_modules(&[parsed])
            .err()
            .ok_or("explicit prelude import acquired an absent namespace owner")?;
        assert!(error.to_string().contains("std.prelude"), "{error}");
        assert!(
            error.to_string().contains("lacks an admitted namespace owner"),
            "{error}"
        );
    }
    Ok(())
}

/// The actual canonical writer preserves explicit prelude imports, including imports inside inline test modules.
#[test]
fn admitted_ordinary_lock_writer_refuses_explicit_and_nested_prelude_without_owner()
-> Result<(), Box<dyn std::error::Error>> {
    for source in [
        "import std.prelude\npub def answer() -> int:\n    return 42\n",
        "from std.prelude import Clone\npub def answer() -> int:\n    return 42\n",
        "from std import prelude\npub def answer() -> int:\n    return 42\n",
        "pub def answer() -> int:\n    return 42\nmodule tests:\n    import std.prelude\n    def test_answer() -> None:\n        pass\n",
    ] {
        let root = tempfile::tempdir()?;
        let (entry, session) = ordinary_session(root.path())?;
        fs::write(&entry, source)?;
        reset_project_lock_collection_metrics();
        let error = publish_oven_project_lock_with_admitted_session(
            root.path(),
            &entry,
            &FeatureSelection::default(),
            &session,
        )
        .err()
        .ok_or("canonical lock accepted an absent explicit prelude owner")?;
        assert!(error.to_string().contains("std.prelude"), "{error}");
        assert_eq!(project_lock_collection_counts(), (1, 0));
        assert!(!root.path().join(LOCK_FILENAME).exists());
    }
    Ok(())
}
