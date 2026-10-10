//! Pinned ordinary ownership and actual-executable source-publication controls.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use incan_lang::lang::standard_packages::{STANDARD_PACKAGE_NAMESPACE_POLICIES, standard_package_namespace_policy};
use oven_model::manifest::ProjectManifest;

use super::{TrustedStandardSourcePublication, TrustedStandardSourcePublicationError};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The ownership table agrees with real authored package declarations, has unique roots and excludes compiler
/// intrinsics.
#[test]
fn dev7_standard_source_policy_matches_authored_declarations() -> TestResult {
    let mut names = BTreeSet::new();
    let mut roots = BTreeSet::new();
    for policy in STANDARD_PACKAGE_NAMESPACE_POLICIES {
        assert!(names.insert(policy.package_name));
        let path = Path::new("compiler-pinned")
            .join(policy.source_directory)
            .join("loaf.toml");
        let parsed = ProjectManifest::from_str(policy.declaration, &path)?;
        let project = parsed.project.as_ref().ok_or("missing authored project identity")?;
        assert_eq!(project.name.as_deref(), Some(policy.package_name));
        assert_eq!(project.version.as_deref(), Some(policy.version));
        for root in policy.namespace_roots {
            assert!(roots.insert(*root), "duplicate namespace owner: {root}");
            assert!(!matches!(*root, "rust" | "builtins"));
        }
    }
    assert_eq!(names.len(), 10);
    assert_eq!(
        standard_package_namespace_policy("incan_stdlib_core")
            .ok_or("missing core policy")?
            .version,
        "0.6.0-dev.6"
    );
    assert_eq!(
        standard_package_namespace_policy("incan_stdlib_system")
            .ok_or("missing system policy")?
            .version,
        "0.5.0"
    );
    assert!(matches!(
        TrustedStandardSourcePublication::discover("stdlib-core"),
        Err(TrustedStandardSourcePublicationError::UnknownPackage { .. })
    ));
    assert!(matches!(
        TrustedStandardSourcePublication::discover("user_incans_stdlib"),
        Err(TrustedStandardSourcePublicationError::UnknownPackage { .. })
    ));
    Ok(())
}

/// Stage a real unit executable and only pinned declarations; caller source coordinates never enter the public factory.
#[cfg(unix)]
fn stage(root: &Path, development: bool, changed: bool) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let executable = root.join(if development {
        "target/compiler-development/bin/incan"
    } else {
        "bin/incan"
    });
    fs::create_dir_all(executable.parent().ok_or("executable parent missing")?)?;
    fs::copy(std::env::current_exe()?, &executable)?;
    let source = root.join(if development { "loaves/stdlib" } else { "stdlib" });
    for name in ["incan_stdlib_system", "incan_stdlib_core"] {
        let policy = standard_package_namespace_policy(name).ok_or("missing policy")?;
        let path = source.join(policy.source_directory).join("loaf.toml");
        fs::create_dir_all(path.parent().ok_or("declaration parent missing")?)?;
        let declaration = if changed {
            format!("{}\n# caller-supplied alteration\n", policy.declaration)
        } else {
            policy.declaration.to_string()
        };
        fs::write(path, declaration)?;
    }
    Ok(executable)
}

/// Run exactly one real child control with hostile source-discovery environment and cwd, all confined to the child.
#[cfg(unix)]
fn child(executable: &Path, expected_package: &Path, mode: &str) -> TestResult {
    let decoy = tempfile::tempdir()?;
    stage(decoy.path(), false, false)?;
    let output = std::process::Command::new(executable)
        .args([
            "--exact",
            "provider::source_policy::tests::dev7_standard_source_policy_actual_executable_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("INCAN_DEV7_STANDARD_SOURCE_CHILD", mode)
        .env("INCAN_DEV7_STANDARD_SOURCE_EXPECTED", expected_package)
        .env("INCAN_SOURCE_ROOT", decoy.path())
        .env("INCAN_STDLIB", decoy.path().join("stdlib"))
        .env("INCAN_INTERNAL_TOOLCHAIN_DATA_ROOT", decoy.path())
        .env("INCAN_INTERNAL_OVEN_RUNTIME_ROOT", decoy.path())
        .env("INCAN_SDK_INVENTORY", decoy.path().join("forged-inventory.json"))
        .current_dir(decoy.path())
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "child failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    if !String::from_utf8_lossy(&output.stdout).contains("1 passed") {
        return Err("child selector did not execute its control".into());
    }
    Ok(())
}

/// Both installed and source-bootstrap executables select real pinned declarations and only their own roots.
#[cfg(unix)]
#[test]
fn dev7_standard_source_policy_actual_public_factory_and_own_namespaces() -> TestResult {
    for development in [false, true] {
        let root = tempfile::tempdir()?;
        let executable = stage(root.path(), development, false)?;
        let package = root.path().join(if development {
            "loaves/stdlib/system"
        } else {
            "stdlib/system"
        });
        child(&executable, &package, "valid")?;
    }
    Ok(())
}

/// Matching public package names/versions in altered declarations cannot select another compiler's publication source.
#[cfg(unix)]
#[test]
fn dev7_standard_source_policy_refuses_altered_and_absent_source() -> TestResult {
    let root = tempfile::tempdir()?;
    let executable = stage(root.path(), false, true)?;
    child(&executable, &root.path().join("stdlib/system"), "invalid-declaration")?;
    fs::remove_dir_all(root.path().join("stdlib"))?;
    child(&executable, root.path(), "absent")?;
    Ok(())
}

/// Held declaration bytes, replacement identity and source coordinates are revalidated on every publication handoff.
#[cfg(unix)]
#[test]
fn dev7_standard_source_policy_revalidates_original_declaration() -> TestResult {
    let root = tempfile::tempdir()?;
    let executable = stage(root.path(), false, false)?;
    child(&executable, &root.path().join("stdlib/system"), "freshness")
}

/// Source selection performs no source writes and needs no preexisting installed package-set descriptor.
#[cfg(unix)]
#[test]
fn dev7_standard_source_policy_selects_read_only_without_installed_set() -> TestResult {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir()?;
    let executable = stage(root.path(), false, false)?;
    let package = root.path().join("stdlib/system");
    let declaration = package.join("loaf.toml");
    let before = fs::read(&declaration)?;
    fs::set_permissions(&declaration, fs::Permissions::from_mode(0o444))?;
    fs::set_permissions(&package, fs::Permissions::from_mode(0o555))?;
    let result = child(&executable, &package, "valid");
    fs::set_permissions(&package, fs::Permissions::from_mode(0o755))?;
    fs::set_permissions(&declaration, fs::Permissions::from_mode(0o644))?;
    result?;
    assert_eq!(fs::read(declaration)?, before);
    assert_eq!(fs::read_dir(package)?.count(), 1);
    assert!(!root.path().join("share/incan/packages/toolchain.json").exists());
    Ok(())
}

/// All actual-child modes enter through the public canonical-executable factory; environment values are assertions
/// only.
#[cfg(unix)]
#[test]
fn dev7_standard_source_policy_actual_executable_child() -> TestResult {
    let Some(mode) = std::env::var_os("INCAN_DEV7_STANDARD_SOURCE_CHILD") else {
        return Ok(());
    };
    let selected = TrustedStandardSourcePublication::discover("incan_stdlib_system");
    if mode == "absent" {
        assert!(selected?.is_none());
        return Ok(());
    }
    if mode == "invalid-declaration" {
        assert!(selected.is_err());
        return Ok(());
    }
    let selected = selected?.ok_or("missing actual trusted source")?;
    let expected = std::env::var_os("INCAN_DEV7_STANDARD_SOURCE_EXPECTED").ok_or("expected package missing")?;
    assert_eq!(
        selected.verified_package_root()?,
        fs::canonicalize(Path::new(&expected))?
    );
    let policy = standard_package_namespace_policy("incan_stdlib_system").ok_or("missing system policy")?;
    assert_eq!(selected.namespace_roots(), policy.namespace_roots);
    assert_eq!(selected.verified_declaration_bytes()?, policy.declaration.as_bytes());
    assert!(!selected.verified_policy_bytes()?.is_empty());
    let core = TrustedStandardSourcePublication::discover("incan_stdlib_core")?.ok_or("missing actual core source")?;
    let core_policy = standard_package_namespace_policy("incan_stdlib_core").ok_or("missing core policy")?;
    core.validate_namespace_roots(
        core_policy.package_name,
        core_policy.version,
        &BTreeSet::from(["runtime".to_string(), "prelude".to_string()]),
    )?;
    assert!(
        core.validate_namespace_roots(core_policy.package_name, policy.version, &BTreeSet::new())
            .is_err()
    );
    assert!(
        core.validate_namespace_roots(
            core_policy.package_name,
            core_policy.version,
            &BTreeSet::from(["io".to_string()])
        )
        .is_err()
    );
    let own = BTreeSet::from(["io".to_string(), "fs".to_string()]);
    for _ in 0..2 {
        selected.validate_namespace_roots(policy.package_name, policy.version, &own)?;
    }
    assert!(
        selected
            .validate_namespace_roots("incan_stdlib_core", policy.version, &own)
            .is_err()
    );
    assert!(
        selected
            .validate_namespace_roots(policy.package_name, "9.9.9", &own)
            .is_err()
    );
    for foreign in ["json", "builtins", "rust", "std", "std::io", "io.private", ""] {
        assert!(
            selected
                .validate_namespace_roots(
                    policy.package_name,
                    policy.version,
                    &BTreeSet::from([foreign.to_string()])
                )
                .is_err()
        );
    }
    if mode == "freshness" {
        let path = selected.verified_package_root()?.join("loaf.toml");
        let original = fs::read(&path)?;
        let modified = fs::metadata(&path)?.modified()?;
        fs::write(&path, String::from_utf8(original.clone())?.replace("0.5.0", "9.9.9"))?;
        fs::File::open(&path)?.set_modified(modified)?;
        assert!(
            selected
                .validate_namespace_roots(policy.package_name, policy.version, &own)
                .is_err()
        );
        fs::write(&path, &original)?;
        fs::File::open(&path)?.set_modified(modified)?;
        selected.validate_namespace_roots(policy.package_name, policy.version, &own)?;
        fs::rename(&path, path.with_extension("retained"))?;
        fs::write(&path, original)?;
        assert!(selected.verified_policy_bytes().is_err());
        assert!(selected.verified_package_root().is_err());
    }
    Ok(())
}
