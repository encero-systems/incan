//! Original-coordinate, no-write and actual-child-executable controls for compiler-owned source selection.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{CompilerOwnedSourceLayout, CompilerOwnedSourceLayoutError, discover_from_executable};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Stage one fixed executable geometry and an intentionally uninterpreted original source declaration.
fn fixture(root: &Path, executable: &str, source: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let executable = root.join(executable);
    fs::create_dir_all(executable.parent().ok_or("executable parent missing")?)?;
    fs::write(&executable, "fixture executable")?;
    let member = root.join(source).join("system/loaf.toml");
    fs::create_dir_all(member.parent().ok_or("member parent missing")?)?;
    fs::write(member, b"original declaration\0bytes\n")?;
    Ok(executable)
}

/// Installed and both checkout generations select only their documented executable-relative source coordinates.
#[cfg(unix)]
#[test]
fn dev7_source_layout_fixed_geometries_and_members() -> TestResult {
    for (executable, source) in [
        ("bin/incan", "stdlib"),
        ("target/debug/incan", "loaves/stdlib"),
        ("target/release/incan", "loaves/stdlib"),
        ("target/compiler-development/bin/incan", "loaves/stdlib"),
    ] {
        let root = tempfile::tempdir()?;
        let executable = fixture(root.path(), executable, source)?;
        let layout = Arc::new(discover_from_executable(&executable)?.ok_or("missing fixed source layout")?);
        assert_eq!(layout.verified_installation_root()?, fs::canonicalize(root.path())?);
        assert_eq!(
            layout.verified_source_root()?,
            fs::canonicalize(root.path().join(source))?
        );
        let member = layout.open_member(Path::new("system/loaf.toml"))?;
        drop(layout);
        assert_eq!(member.verified_bytes()?, b"original declaration\0bytes\n");
        assert_eq!(
            member.path(),
            fs::canonicalize(root.path().join(source).join("system/loaf.toml"))?
        );
    }
    Ok(())
}

/// Current bytes are always observed: preserved times cannot hide edits, and equal-byte replacement is another owner.
#[cfg(unix)]
#[test]
fn dev7_source_layout_member_tamper_restoration_and_replacement() -> TestResult {
    let root = tempfile::tempdir()?;
    let executable = fixture(root.path(), "bin/incan", "stdlib")?;
    let layout = Arc::new(discover_from_executable(&executable)?.ok_or("missing source layout")?);
    let member = layout.open_member(Path::new("system/loaf.toml"))?;
    let path = member.path().to_path_buf();
    let modified = fs::metadata(&path)?.modified()?;
    fs::write(&path, b"changed declaration\0bytes\n")?;
    fs::File::open(&path)?.set_modified(modified)?;
    assert!(member.verified_bytes().is_err());
    fs::write(&path, b"original declaration\0bytes\n")?;
    fs::File::open(&path)?.set_modified(modified)?;
    assert_eq!(member.verified_bytes()?, b"original declaration\0bytes\n");
    fs::rename(&path, path.with_extension("retained"))?;
    fs::write(&path, b"original declaration\0bytes\n")?;
    fs::File::open(&path)?.set_modified(modified)?;
    assert!(member.verified_bytes().is_err());
    Ok(())
}

/// Replaced original source/package directories refuse even when the declaration's original inode is moved back.
#[cfg(unix)]
#[test]
fn dev7_source_layout_directory_replacement_and_relocation() -> TestResult {
    let root = tempfile::tempdir()?;
    let executable = fixture(root.path(), "bin/incan", "stdlib")?;
    let layout = Arc::new(discover_from_executable(&executable)?.ok_or("missing source layout")?);
    let member = layout.open_member(Path::new("system/loaf.toml"))?;
    let package = root.path().join("stdlib/system");
    let held = root.path().join("stdlib/held-system");
    fs::rename(&package, &held)?;
    fs::create_dir(&package)?;
    fs::rename(held.join("loaf.toml"), package.join("loaf.toml"))?;
    assert!(member.verified_bytes().is_err());
    fs::remove_dir(&held)?;
    let source = root.path().join("stdlib");
    fs::rename(&source, root.path().join("held-stdlib"))?;
    fs::create_dir(&source)?;
    assert!(layout.verify().is_err());
    let fresh = discover_from_executable(&executable)?.ok_or("replacement layout missing")?;
    assert!(fresh.verify().is_ok());
    let relocated = tempfile::tempdir()?;
    let moved = relocated.path().join("relocated");
    fs::rename(root.path(), &moved)?;
    assert!(fresh.verify().is_err());
    assert!(discover_from_executable(&moved.join("bin/incan"))?.is_some());
    Ok(())
}

/// Traversal, symlink members and source roots cannot escape the fixed canonical executable installation.
#[cfg(unix)]
#[test]
fn dev7_source_layout_refuses_symlink_escape_and_traversal() -> TestResult {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir()?;
    let executable = fixture(root.path(), "bin/incan", "stdlib")?;
    let layout = Arc::new(discover_from_executable(&executable)?.ok_or("missing source layout")?);
    for relative in ["", "../stdlib/system/loaf.toml", "/etc/passwd", "system/./loaf.toml"] {
        assert!(layout.open_member(Path::new(relative)).is_err());
    }
    let package = root.path().join("stdlib/system");
    fs::rename(&package, root.path().join("outside"))?;
    symlink(root.path().join("outside"), &package)?;
    assert!(layout.open_member(Path::new("system/loaf.toml")).is_err());
    fs::remove_file(&package)?;
    fs::rename(root.path().join("outside"), &package)?;
    let path = package.join("loaf.toml");
    fs::rename(&path, package.join("original"))?;
    symlink(package.join("original"), &path)?;
    assert!(layout.open_member(Path::new("system/loaf.toml")).is_err());
    fs::rename(root.path().join("stdlib"), root.path().join("external-source"))?;
    symlink(root.path().join("external-source"), root.path().join("stdlib"))?;
    assert!(discover_from_executable(&executable).is_err());
    assert!(layout.verify().is_err());
    assert!(discover_from_executable(&root.path().join("bin/../bin/incan")).is_err());
    Ok(())
}

/// Competing and present-invalid source candidates refuse rather than falling through to another geometry.
#[cfg(unix)]
#[test]
fn dev7_source_layout_refuses_competitors_and_present_invalid() -> TestResult {
    let root = tempfile::tempdir()?;
    let executable = fixture(root.path(), "target/compiler-development/bin/incan", "loaves/stdlib")?;
    let layout = discover_from_executable(&executable)?.ok_or("missing checkout layout")?;
    let competitor = root.path().join("target/compiler-development/stdlib");
    fs::create_dir(&competitor)?;
    assert!(matches!(
        discover_from_executable(&executable),
        Err(CompilerOwnedSourceLayoutError::CompetingRoots { .. })
    ));
    assert!(layout.verify().is_err());
    fs::remove_dir(&competitor)?;
    fs::write(&competitor, "present invalid candidate")?;
    assert!(matches!(
        discover_from_executable(&executable),
        Err(CompilerOwnedSourceLayoutError::Invalid { .. })
    ));
    assert!(layout.verify().is_err());
    Ok(())
}

/// A symlink launcher beside forged sources still selects the actual executable installation; retargeting refuses.
#[cfg(unix)]
#[test]
fn dev7_source_layout_ignores_launcher_sources_and_refuses_retarget() -> TestResult {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir()?;
    let genuine = fixture(&root.path().join("genuine"), "bin/incan", "stdlib")?;
    let other = fixture(&root.path().join("other"), "bin/incan", "stdlib")?;
    let launcher = root.path().join("launcher/bin/incan");
    fixture(&root.path().join("launcher"), "bin/placeholder", "stdlib")?;
    symlink(&genuine, &launcher)?;
    let layout = discover_from_executable(&launcher)?.ok_or("missing canonical source layout")?;
    assert_eq!(
        layout.verified_source_root()?,
        fs::canonicalize(root.path().join("genuine/stdlib"))?
    );
    fs::remove_file(&launcher)?;
    symlink(other, &launcher)?;
    assert!(layout.verify().is_err());
    fs::remove_file(root.path().join("genuine/stdlib/system/loaf.toml"))?;
    fs::remove_dir(root.path().join("genuine/stdlib/system"))?;
    fs::remove_dir(root.path().join("genuine/stdlib"))?;
    fs::remove_file(&launcher)?;
    symlink(genuine, &launcher)?;
    assert!(discover_from_executable(&launcher)?.is_none());
    Ok(())
}

/// Actual child execution exercises the public factory read-only, ignoring caller environment/cwd decoys.
#[cfg(unix)]
#[test]
fn dev7_source_layout_actual_executable_read_only_and_ambient_refusal() -> TestResult {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    let root = tempfile::tempdir()?;
    let executable = fixture(root.path(), "bin/incan", "stdlib")?;
    fs::copy(std::env::current_exe()?, &executable)?;
    let decoy = tempfile::tempdir()?;
    fixture(decoy.path(), "bin/incan", "stdlib")?;
    let source = root.path().join("stdlib");
    let package = source.join("system");
    let member = package.join("loaf.toml");
    let before = fs::read(&member)?;
    fs::set_permissions(&member, fs::Permissions::from_mode(0o444))?;
    fs::set_permissions(&package, fs::Permissions::from_mode(0o555))?;
    fs::set_permissions(&source, fs::Permissions::from_mode(0o555))?;
    let output = Command::new(&executable)
        .args([
            "--exact",
            "toolchain_layout::source_layout::tests::dev7_source_layout_actual_executable_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("INCAN_DEV7_SOURCE_LAYOUT_CHILD", "present")
        .env("INCAN_DEV7_SOURCE_LAYOUT_EXPECTED", &source)
        .env("INCAN_STDLIB", decoy.path())
        .env("INCAN_SOURCE_ROOT", decoy.path())
        .env("INCAN_INTERNAL_TOOLCHAIN_DATA_ROOT", decoy.path())
        .current_dir(decoy.path())
        .output();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o755))?;
    fs::set_permissions(&package, fs::Permissions::from_mode(0o755))?;
    fs::set_permissions(&member, fs::Permissions::from_mode(0o644))?;
    let output = output?;
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    assert_eq!(fs::read(&member)?, before);
    assert_eq!(fs::read_dir(&source)?.count(), 1);
    assert_eq!(fs::read_dir(&package)?.count(), 1);
    fs::remove_file(member)?;
    fs::remove_dir(package)?;
    fs::remove_dir(source)?;
    let output = Command::new(&executable)
        .args([
            "--exact",
            "toolchain_layout::source_layout::tests::dev7_source_layout_actual_executable_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("INCAN_DEV7_SOURCE_LAYOUT_CHILD", "absent")
        .env("INCAN_STDLIB", decoy.path())
        .env("INCAN_SOURCE_ROOT", decoy.path())
        .current_dir(decoy.path())
        .output()?;
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    Ok(())
}

/// The child selects only its actual executable; expected-output environment never supplies authority.
#[cfg(unix)]
#[test]
fn dev7_source_layout_actual_executable_child() -> TestResult {
    let Some(mode) = std::env::var_os("INCAN_DEV7_SOURCE_LAYOUT_CHILD") else {
        return Ok(());
    };
    let selected = CompilerOwnedSourceLayout::discover()?;
    if mode == "absent" {
        assert!(selected.is_none());
        return Ok(());
    }
    let layout = Arc::new(selected.ok_or("actual source layout missing")?);
    let expected = std::env::var_os("INCAN_DEV7_SOURCE_LAYOUT_EXPECTED").ok_or("expected source missing")?;
    assert_eq!(layout.verified_source_root()?, fs::canonicalize(Path::new(&expected))?);
    let member = layout.open_member(Path::new("system/loaf.toml"))?;
    for _ in 0..2 {
        assert_eq!(member.verified_bytes()?, b"original declaration\0bytes\n");
    }
    Ok(())
}

/// Missing stable platform identity is an explicit unsupported gate.
#[cfg(not(unix))]
#[test]
fn dev7_source_layout_refuses_unsupported_identity() -> TestResult {
    let root = tempfile::tempdir()?;
    let executable = fixture(root.path(), "bin/incan", "stdlib")?;
    assert!(matches!(
        discover_from_executable(&executable),
        Err(CompilerOwnedSourceLayoutError::UnsupportedFileIdentity)
    ));
    Ok(())
}
