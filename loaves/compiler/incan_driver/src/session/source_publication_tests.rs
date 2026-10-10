//! Real executable-selected source authority in ordinary sessions, distinct from installed dependency grants.

use std::collections::BTreeSet;
use std::fs;
use std::sync::Arc;

use incan_frontend::provider::source_policy::TrustedStandardSourcePublication;
use incan_lang::lang::standard_packages::standard_package_namespace_policy;
use incan_provider::FeatureSelection;
use oven_store::store::OvenStoreLimits;

use super::CompilationSession;
use crate::build::library_dependencies::PreparedLibraryDependencies;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A real installed child selects pinned source while ambient discovery points elsewhere.
#[cfg(unix)]
#[test]
fn dev7_standard_source_session_retains_own_authority_and_refuses_substitution() -> TestResult {
    let root = tempfile::tempdir()?;
    let package = root.path().join("stdlib/system");
    fs::create_dir_all(package.join("src"))?;
    let policy = standard_package_namespace_policy("incan_stdlib_system").ok_or("missing system policy")?;
    fs::write(package.join("loaf.toml"), policy.declaration)?;
    fs::write(package.join("src/lib.incn"), "from std.io import value\n")?;
    fs::write(package.join("src/io.incn"), "def value() -> int:\n    return 42\n")?;
    let executable = root.path().join("bin/incan");
    fs::create_dir_all(executable.parent().ok_or("missing executable parent")?)?;
    fs::copy(std::env::current_exe()?, &executable)?;
    let decoy = tempfile::tempdir()?;
    let output = std::process::Command::new(executable)
        .args([
            "--exact",
            "session::source_publication_tests::dev7_standard_source_session_actual_executable_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("INCAN_DEV7_STANDARD_SESSION_CHILD", "1")
        .env("INCAN_SOURCE_ROOT", decoy.path())
        .env("INCAN_STDLIB", decoy.path())
        .env("INCAN_SDK_INVENTORY", decoy.path().join("forged-inventory.json"))
        .current_dir(decoy.path())
        .output()?;
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    Ok(())
}

/// Actual ordinary-session construction retains source and dependency owners across module projections and rejects
/// foreign roots, copied packages, mixed SDK authority and equal-byte declaration replacement.
#[cfg(unix)]
#[test]
fn dev7_standard_source_session_actual_executable_child() -> TestResult {
    if std::env::var_os("INCAN_DEV7_STANDARD_SESSION_CHILD").is_none() {
        return Ok(());
    }
    let source = Arc::new(TrustedStandardSourcePublication::discover("incan_stdlib_system")?.ok_or("missing source")?);
    let root = source.verified_package_root()?.to_path_buf();
    let entry = root.join("src/lib.incn");
    let features = FeatureSelection::default();
    let dependencies = Arc::new(PreparedLibraryDependencies::admit(
        &[],
        "aarch64-apple-darwin",
        "ordinary-source-session-control",
        OvenStoreLimits::new(16 << 20, 16 << 20, 16 << 20),
    )?);
    let session = CompilationSession::discover_with_admitted_standard_source(
        &entry,
        &features,
        Arc::clone(&dependencies),
        Arc::clone(&source),
    )?;
    assert!(session.sdk_inventory.is_none());
    assert!(session.sdk_components.is_none());
    assert!(Arc::ptr_eq(
        session.admitted_library_dependencies().ok_or("missing dependencies")?,
        &dependencies
    ));
    assert!(session.provider_plan.public_artifacts().next().is_none());
    let modules = crate::modules::collect_modules_detailed_with_session(entry.clone(), &session)
        .map_err(|failure| failure.render_human())?;
    assert_eq!(modules.len(), 2);
    let owned = modules
        .iter()
        .find(|module| module.path_segments == ["io"])
        .ok_or("own source import did not retain physical identity")?;
    assert_eq!(owned.file_path, root.join("src/io.incn"));
    assert!(owned.source.contains("return 42"));
    let owned_path = root.join("src/io.incn");
    fs::rename(&owned_path, root.join("retained-io.incn"))?;
    assert!(crate::modules::collect_modules_detailed_with_session(entry.clone(), &session).is_err());
    fs::rename(root.join("retained-io.incn"), &owned_path)?;
    let escaped = tempfile::tempdir()?;
    fs::write(escaped.path().join("io.incn"), "def value() -> int:\n    return 999\n")?;
    fs::rename(&owned_path, root.join("retained-io.incn"))?;
    std::os::unix::fs::symlink(escaped.path().join("io.incn"), &owned_path)?;
    assert!(crate::modules::collect_modules_detailed_with_session(entry.clone(), &session).is_err());
    fs::remove_file(&owned_path)?;
    fs::rename(root.join("retained-io.incn"), &owned_path)?;
    let own: BTreeSet<Vec<String>> = [vec!["std".into(), "io".into()]].into();
    let first = session.provider_plan_for_used_module_paths(own.clone())?;
    let repeat = session.provider_plan_for_used_module_paths(own.clone())?;
    assert!(Arc::ptr_eq(&first, &repeat));
    assert!(Arc::ptr_eq(
        first.standard_source_publication().ok_or("missing retained source")?,
        &source
    ));
    assert!(first.bootstrap_owns_sdk_module(&["std".into(), "io".into(), "private".into()]));
    assert!(!first.bootstrap_owns_sdk_module(&["std".into(), "json".into()]));
    assert!(!first.bootstrap_owns_sdk_module(&["pub".into(), "io".into()]));
    assert_ne!(
        first.semantic_projection_persistent_key()?,
        dependencies.provider_plan().semantic_projection_persistent_key()?
    );
    assert!(
        session
            .provider_plan_for_used_module_paths([vec!["std".into(), "json".into()]].into())
            .is_err()
    );

    let policy = standard_package_namespace_policy("incan_stdlib_system").ok_or("missing policy")?;
    let copied = tempfile::tempdir()?;
    fs::create_dir(copied.path().join("src"))?;
    fs::write(copied.path().join("loaf.toml"), policy.declaration)?;
    fs::write(copied.path().join("src/lib.incn"), fs::read(&entry)?)?;
    assert!(
        CompilationSession::discover_with_admitted_standard_source(
            &copied.path().join("src/lib.incn"),
            &features,
            Arc::clone(&dependencies),
            Arc::clone(&source),
        )
        .is_err()
    );
    let competing = first
        .as_ref()
        .clone()
        .with_bootstrap_sdk_namespace_roots(["json".into()]);
    assert!(competing.verify_standard_source_publication().is_err());
    assert!(
        dependencies
            .provider_plan()
            .as_ref()
            .clone()
            .with_standard_source_publication(Arc::clone(&source), &root, "incan_stdlib_core", "0.6.0-dev.6",)
            .is_err()
    );

    let declaration = root.join("loaf.toml");
    let original = fs::read(&declaration)?;
    fs::rename(&declaration, root.join("retained-loaf.toml"))?;
    fs::write(&declaration, original)?;
    assert!(session.provider_plan_for_used_module_paths(own).is_err());
    assert!(source.verified_package_root().is_err());
    Ok(())
}
