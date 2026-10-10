//! Real filesystem and actual-executable controls for the installed data anchor.

use std::fs;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use super::CompilerOwnedInstalledData;
use super::{COMPILER_OWNED_PACKAGE_DESCRIPTOR, CompilerOwnedInstalledDataError, discover_from_executable};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Build the fixed layout without interpreting the intentionally opaque descriptor bytes.
fn installation(root: &Path, bytes: Option<&[u8]>) -> Result<PathBuf, Box<dyn std::error::Error>> {
    fs::create_dir_all(root.join("bin"))?;
    let executable = root.join("bin/incan");
    fs::write(&executable, "test executable")?;
    if let Some(bytes) = bytes {
        descriptor(root, bytes)?;
    }
    Ok(executable)
}

/// Publish exact fixture bytes at the stable relative descriptor coordinate.
fn descriptor(root: &Path, bytes: &[u8]) -> TestResult {
    let path = root.join(COMPILER_OWNED_PACKAGE_DESCRIPTOR);
    let parent = path.parent().ok_or("descriptor parent absent")?;
    fs::create_dir_all(parent)?;
    fs::write(path, bytes)?;
    Ok(())
}

/// An absent descriptor is absence; a present invalid descriptor and traversal are explicit refusals.
#[cfg(unix)]
#[test]
fn dev7_installed_data_anchor_refuses_invalid_missing_and_traversal() -> TestResult {
    let temporary = tempfile::tempdir()?;
    let root = temporary.path().join("install");
    let executable = installation(&root, None)?;
    assert!(discover_from_executable(&executable)?.is_none());
    fs::create_dir_all(root.join(COMPILER_OWNED_PACKAGE_DESCRIPTOR))?;
    assert!(discover_from_executable(&executable).is_err());
    fs::remove_dir(root.join(COMPILER_OWNED_PACKAGE_DESCRIPTOR))?;
    descriptor(&root, b"original opaque bytes")?;
    assert!(discover_from_executable(&root.join("bin/../bin/incan")).is_err());
    assert!(discover_from_executable(Path::new("bin/incan")).is_err());
    let anchor = discover_from_executable(&executable)?.ok_or("missing anchor")?;
    fs::remove_file(root.join(COMPILER_OWNED_PACKAGE_DESCRIPTOR))?;
    assert!(anchor.verified().is_err());
    Ok(())
}

/// Fresh discovery follows a relocated installation; a previously retained absolute coordinate cannot migrate itself.
#[cfg(unix)]
#[test]
fn dev7_installed_data_anchor_relocation_preserves_bytes_and_refuses_stale_coordinate() -> TestResult {
    let temporary = tempfile::tempdir()?;
    let root = temporary.path().join("first");
    let executable = installation(&root, Some(b"original\0opaque\nbytes"))?;
    let anchor = discover_from_executable(&executable)?.ok_or("missing first anchor")?;
    let identity = anchor.verified()?.descriptor_identity().to_owned();
    let relocated = temporary.path().join("relocated");
    fs::rename(root, &relocated)?;
    assert!(anchor.verified().is_err());
    let fresh = discover_from_executable(&relocated.join("bin/incan"))?.ok_or("missing relocated anchor")?;
    let view = fresh.verified()?;
    assert_eq!(view.descriptor_identity(), identity);
    assert_eq!(view.descriptor_bytes(), b"original\0opaque\nbytes");
    assert_eq!(view.installation_root(), fs::canonicalize(&relocated)?);
    assert_eq!(
        view.descriptor_path(),
        fs::canonicalize(relocated.join(COMPILER_OWNED_PACKAGE_DESCRIPTOR))?
    );
    Ok(())
}

/// Preserved timestamps cannot hide changed bytes, and equal bytes in a replacement file cannot replace the held owner.
#[cfg(unix)]
#[test]
fn dev7_installed_data_anchor_tamper_restoration_and_equal_byte_substitution() -> TestResult {
    let temporary = tempfile::tempdir()?;
    let root = temporary.path().join("install");
    let executable = installation(&root, Some(b"original bytes"))?;
    let path = root.join(COMPILER_OWNED_PACKAGE_DESCRIPTOR);
    let anchor = discover_from_executable(&executable)?.ok_or("missing anchor")?;
    let modified = fs::metadata(&path)?.modified()?;
    fs::write(&path, b"different data")?;
    fs::File::open(&path)?.set_modified(modified)?;
    assert!(anchor.verified().is_err());
    fs::write(&path, b"original bytes")?;
    fs::File::open(&path)?.set_modified(modified)?;
    assert_eq!(anchor.verified()?.descriptor_bytes(), b"original bytes");
    let retained = path.with_extension("original");
    fs::rename(&path, &retained)?;
    fs::write(&path, b"original bytes")?;
    fs::File::open(&path)?.set_modified(modified)?;
    assert!(anchor.verified().is_err());
    assert_eq!(fs::read(retained)?, b"original bytes");
    Ok(())
}

/// A newly introduced second descriptor cannot be hidden by candidate search order or an existing anchor.
#[cfg(unix)]
#[test]
fn dev7_installed_data_anchor_refuses_competing_roots() -> TestResult {
    let temporary = tempfile::tempdir()?;
    let root = temporary.path().join("install");
    let executable = installation(&root, Some(b"same bytes"))?;
    let anchor = discover_from_executable(&executable)?.ok_or("missing anchor")?;
    descriptor(&root.join("bin"), b"same bytes")?;
    assert!(matches!(
        anchor.verified(),
        Err(CompilerOwnedInstalledDataError::CompetingRoots { .. })
    ));
    assert!(matches!(
        discover_from_executable(&executable),
        Err(CompilerOwnedInstalledDataError::CompetingRoots { .. })
    ));
    Ok(())
}

/// Neither a descriptor symlink nor an escaping directory is followed, even when another candidate is valid.
#[cfg(unix)]
#[test]
fn dev7_installed_data_anchor_refuses_symlink_escape_and_present_invalid_candidate() -> TestResult {
    use std::os::unix::fs::symlink;

    let temporary = tempfile::tempdir()?;
    let root = temporary.path().join("install");
    let executable = installation(&root, Some(b"original bytes"))?;
    let outside = temporary.path().join("outside.json");
    fs::write(&outside, b"original bytes")?;
    let path = root.join(COMPILER_OWNED_PACKAGE_DESCRIPTOR);
    let anchor = discover_from_executable(&executable)?.ok_or("missing anchor")?;
    fs::rename(&path, path.with_extension("original"))?;
    symlink(&outside, &path)?;
    assert!(anchor.verified().is_err());
    assert!(discover_from_executable(&executable).is_err());
    fs::remove_file(&path)?;
    fs::write(&path, b"original bytes")?;
    let outside_share = temporary.path().join("outside-share");
    fs::create_dir(&outside_share)?;
    symlink(&outside_share, root.join("bin/share"))?;
    assert!(discover_from_executable(&executable).is_err());
    Ok(())
}

/// An executable alias's forged descriptor cannot issue an anchor; only the canonical installed executable owns data.
#[cfg(unix)]
#[test]
fn dev7_installed_data_anchor_accepts_executable_alias_and_refuses_retarget() -> TestResult {
    use std::os::unix::fs::symlink;

    let temporary = tempfile::tempdir()?;
    let root = temporary.path().join("install");
    let executable = installation(&root, Some(b"original bytes"))?;
    let launcher = temporary.path().join("launcher/bin/incan");
    fs::create_dir_all(launcher.parent().ok_or("missing launcher parent")?)?;
    symlink(&executable, &launcher)?;
    descriptor(&temporary.path().join("launcher"), b"forged launcher bytes")?;
    let anchor = discover_from_executable(&launcher)?.ok_or("missing aliased anchor")?;
    assert_eq!(anchor.verified()?.installation_root(), fs::canonicalize(&root)?);
    let replacement = installation(&temporary.path().join("replacement"), Some(b"original bytes"))?;
    fs::remove_file(&launcher)?;
    symlink(replacement, &launcher)?;
    assert!(anchor.verified().is_err());
    Ok(())
}

/// Exercise the public actual-current-executable factory with a copied real test executable and read-only layout.
#[cfg(unix)]
#[test]
fn dev7_installed_data_anchor_discovers_actual_executable_read_only() -> TestResult {
    use std::os::unix::fs::PermissionsExt;

    let temporary = tempfile::tempdir()?;
    let root = temporary.path().join("install");
    let executable = installation(&root, Some(b"original\0opaque\nbytes"))?;
    fs::copy(std::env::current_exe()?, &executable)?;
    let mut paths = fixture_paths(&root)?;
    for path in &paths {
        let mode = if path.is_dir() || *path == executable {
            0o555
        } else {
            0o444
        };
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    }
    let before = fixture_state(&root)?;
    let result = child(&root, false);
    let after = fixture_state(&root)?;
    paths.reverse();
    for path in paths {
        fs::set_permissions(
            &path,
            fs::Permissions::from_mode(if path.is_dir() { 0o755 } else { 0o644 }),
        )?;
    }
    result?;
    assert_eq!(before, after, "discovery and repeat verification wrote installed state");
    Ok(())
}

/// Scheduler, SDK and stdlib environment roots cannot fill an absent actual executable-relative descriptor.
#[cfg(unix)]
#[test]
fn dev7_installed_data_anchor_ignores_ambient_data_roots() -> TestResult {
    let temporary = tempfile::tempdir()?;
    let root = temporary.path().join("install");
    let executable = installation(&root, None)?;
    fs::copy(std::env::current_exe()?, executable)?;
    child(&root, true)
}

/// Execute the public factory in a child whose actual executable and ambient decoys are controlled independently.
#[cfg(unix)]
fn child(root: &Path, absent: bool) -> TestResult {
    use std::process::Command;

    let decoy = tempfile::tempdir()?;
    installation(decoy.path(), Some(b"ambient bytes must not be selected"))?;
    let output = Command::new(root.join("bin/incan"))
        .args([
            "--exact",
            "toolchain_layout::installed_data::tests::dev7_installed_data_anchor_actual_executable_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(
            "INCAN_DEV7_INSTALLED_DATA_CHILD",
            if absent { "absent" } else { "present" },
        )
        .env("INCAN_DEV7_INSTALLED_DATA_EXPECTED_ROOT", root)
        .env("INCAN_INTERNAL_TOOLCHAIN_DATA_ROOT", decoy.path())
        .env("INCAN_INTERNAL_OVEN_LOAF_EXECUTION", "1")
        .env("INCAN_INTERNAL_OVEN_RUNTIME_ROOT", decoy.path())
        .env(
            "INCAN_SDK_INVENTORY",
            decoy.path().join(COMPILER_OWNED_PACKAGE_DESCRIPTOR),
        )
        .env("INCAN_STDLIB", decoy.path())
        .env("INCAN_STDLIB_DIR", decoy.path())
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

/// This selected child calls only the public constructor; environment data is expected-output evidence, not authority.
#[cfg(unix)]
#[test]
fn dev7_installed_data_anchor_actual_executable_child() -> TestResult {
    let Some(mode) = std::env::var_os("INCAN_DEV7_INSTALLED_DATA_CHILD") else {
        return Ok(());
    };
    let selected = CompilerOwnedInstalledData::discover()?;
    if mode == "absent" {
        assert!(selected.is_none());
    } else {
        let anchor = selected.ok_or("missing current-executable anchor")?;
        for _ in 0..2 {
            let view = anchor.verified()?;
            let expected =
                std::env::var_os("INCAN_DEV7_INSTALLED_DATA_EXPECTED_ROOT").ok_or("missing expected root")?;
            assert_eq!(view.installation_root(), fs::canonicalize(Path::new(&expected))?);
            assert_eq!(view.descriptor_bytes(), b"original\0opaque\nbytes");
            assert_eq!(
                view.descriptor_identity(),
                crate::digest::digest_bytes(view.descriptor_bytes())
            );
        }
    }
    Ok(())
}

/// Snapshot all installed entries without treating access timestamps as product writes.
#[cfg(unix)]
fn fixture_paths(root: &Path) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let mut paths = vec![root.to_path_buf()];
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            paths.extend(fixture_paths(&entry.path())?);
        } else {
            paths.push(entry.path());
        }
    }
    paths.sort();
    Ok(paths)
}

/// Capture topology, permissions, modification time and size before and after actual child discovery.
#[cfg(unix)]
fn fixture_state(root: &Path) -> Result<Vec<(PathBuf, u32, u64, std::time::SystemTime)>, Box<dyn std::error::Error>> {
    use std::os::unix::fs::PermissionsExt;

    fixture_paths(root)?
        .into_iter()
        .map(|path| {
            let metadata = fs::metadata(&path)?;
            Ok((
                path,
                metadata.permissions().mode(),
                metadata.len(),
                metadata.modified()?,
            ))
        })
        .collect()
}

/// Unsupported platforms refuse rather than pretending equal bytes or timestamps identify an original file.
#[cfg(not(unix))]
#[test]
fn dev7_installed_data_anchor_refuses_unsupported_file_identity() -> TestResult {
    let temporary = tempfile::tempdir()?;
    let executable = installation(temporary.path(), Some(b"original bytes"))?;
    assert!(matches!(
        discover_from_executable(&executable),
        Err(CompilerOwnedInstalledDataError::UnsupportedFileIdentity)
    ));
    Ok(())
}
