//! Actual pinned Git transport, refusal, framing and child-lifetime controls.

use super::{IndexBatchWork, PinnedIndexBatch, batch_support, read_object};
use crate::sdk_closure::{Error, index_file};
use std::io::Cursor;
use std::path::Path;
use std::process::{Command, ExitStatus, Output};

/// Run fixture Git without changing shared process environment or accepting unsuccessful commands.
fn git(root: &Path, arguments: &[&str]) -> Result<Vec<u8>, Error> {
    let output = Command::new("git").arg("-C").arg(root).args(arguments).output()?;
    if !output.status.success() {
        return Err(format!("fixture Git failed: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    Ok(output.stdout)
}

/// Create binary and unusual-path blobs under a real immutable commit; subsequent worktree edits are irrelevant.
fn fixture() -> Result<(tempfile::TempDir, String), Error> {
    let root = tempfile::tempdir()?;
    git(root.path(), &["init", "--quiet"])?;
    std::fs::create_dir(root.path().join("directory"))?;
    std::fs::write(root.path().join("directory/file"), b"first\0\nlast\xff")?;
    std::fs::write(root.path().join("with\nnewline"), b"other\n\0bytes")?;
    git(root.path(), &["add", "."])?;
    git(
        root.path(),
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgSign=false",
            "commit",
            "--quiet",
            "-m",
            "immutable blobs",
        ],
    )?;
    let pin = String::from_utf8(git(root.path(), &["rev-parse", "HEAD"])?)?
        .trim()
        .to_string();
    std::fs::write(root.path().join("directory/file"), b"mutable worktree substitute")?;
    Ok((root, pin))
}

/// Supported batching reads each unique pinned blob once in one persistent process, including newline paths.
#[test]
fn dev7_pinned_index_batch_raw_bytes_and_actual_work() -> Result<(), Error> {
    let (root, pin) = fixture()?;
    let mut reader = PinnedIndexBatch::open(root.path(), &pin)?;
    for path in ["directory/file", "with\nnewline"] {
        let expected = index_file(root.path(), &pin, path)?;
        assert_eq!(reader.read(path)?, expected);
        assert_eq!(reader.read(path)?, expected);
    }
    assert_eq!(reader.adoption_paths()?, Vec::<String>::new());
    assert_eq!(reader.adoption_paths()?, Vec::<String>::new());
    assert_eq!(reader.work().file_requests, 4);
    assert_eq!(reader.work().blob_reads, 2);
    assert_eq!(reader.work().cache_hits, 2);
    assert_eq!(
        reader.work().blob_bytes,
        b"first\0\nlast\xff".len() + b"other\n\0bytes".len()
    );
    if reader.batch {
        assert_eq!(reader.work().processes, 3); // Capability, persistent child and one event listing.
        assert_eq!(reader.work().requests, 3); // Commit plus two distinct blobs.
    } else {
        assert_eq!(reader.work().processes, 7); // Capability, admission, two canonical checks/reads, listing.
        assert_eq!(reader.work().requests, 0);
    }
    reader.finish()?;
    assert!(reader.child.is_none());
    assert!(reader.read("directory/file").is_err());
    reader.finish()?;
    git(root.path(), &["add", "."])?;
    git(
        root.path(),
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgSign=false",
            "commit",
            "--quiet",
            "-m",
            "new pinned source",
        ],
    )?;
    let changed = String::from_utf8(git(root.path(), &["rev-parse", "HEAD"])?)?
        .trim()
        .to_string();
    let mut next = PinnedIndexBatch::open(root.path(), &changed)?;
    assert_eq!(next.read("directory/file")?, b"mutable worktree substitute");
    assert_eq!(index_file(root.path(), &pin, "directory/file")?, b"first\0\nlast\xff");
    next.finish()?;
    Ok(())
}

/// Older-Git selection keeps exact raw bytes and commit checks; cached repeats launch no extra transport.
#[test]
fn dev7_pinned_index_batch_unsupported_transport_fallback() -> Result<(), Error> {
    let (root, pin) = fixture()?;
    let mut reader = PinnedIndexBatch::with_support(root.path(), &pin, false, IndexBatchWork::default())?;
    assert_eq!(reader.work().processes, 1);
    for path in ["directory/file", "with\nnewline"] {
        assert_eq!(reader.read(path)?, index_file(root.path(), &pin, path)?);
        reader.read(path)?;
    }
    assert_eq!(reader.work().processes, 5);
    assert_eq!(reader.work().requests, 0);
    assert_eq!(reader.work().file_requests, 4);
    assert_eq!(reader.work().blob_reads, 2);
    assert_eq!(reader.work().cache_hits, 2);
    reader.finish()?;
    Ok(())
}

/// Neither transport accepts tree-as-file, absent paths, invalid pins or path/protocol injection.
#[test]
fn dev7_pinned_index_batch_pin_path_and_object_refusals() -> Result<(), Error> {
    let (root, pin) = fixture()?;
    let tree = String::from_utf8(git(root.path(), &["rev-parse", "HEAD^{tree}"])?)?
        .trim()
        .to_string();
    for batch in [false, true] {
        // Force batch only when capability selection supports it, preserving the old-Git product contract.
        let capability = PinnedIndexBatch::open(root.path(), &pin)?;
        if batch && !capability.batch {
            continue;
        }
        drop(capability);
        for invalid in ["HEAD", &tree, "0000000000000000000000000000000000000000"] {
            assert!(PinnedIndexBatch::with_support(root.path(), invalid, batch, IndexBatchWork::default()).is_err());
        }
        for path in ["directory", "missing\npath"] {
            assert!(index_file(root.path(), &pin, path).is_err());
            let mut reader = PinnedIndexBatch::with_support(root.path(), &pin, batch, IndexBatchWork::default())?;
            assert!(reader.read(path).is_err());
            assert!(reader.finish().is_err());
            assert!(reader.child.is_none());
        }
        let mut reader = PinnedIndexBatch::with_support(root.path(), &pin, batch, IndexBatchWork::default())?;
        for path in [
            "../file",
            "/absolute",
            "Cargo.toml",
            "nested/Cargo.lock",
            "directory/file\0HEAD",
        ] {
            assert!(reader.read(path).is_err());
        }
        assert_eq!(reader.work().file_requests, 0);
        assert_eq!(reader.read("directory/file")?, b"first\0\nlast\xff");
        reader.finish()?;
    }
    Ok(())
}

/// Early return kills and reaps the real persistent child; no background Git process survives command ownership.
#[cfg(unix)]
#[test]
fn dev7_pinned_index_batch_child_lifetime() -> Result<(), Error> {
    let (root, pin) = fixture()?;
    let reader = PinnedIndexBatch::open(root.path(), &pin)?;
    let Some(child) = reader.child.as_ref() else {
        return Ok(());
    };
    let pid = child.id().to_string();
    assert!(Command::new("kill").args(["-0", &pid]).output()?.status.success());
    drop(reader);
    assert!(!Command::new("kill").args(["-0", &pid]).output()?.status.success());
    Ok(())
}

/// Usage refusals select fallback; fatal repository failures never become successful capability selection.
#[cfg(unix)]
#[test]
fn dev7_pinned_index_batch_capability_and_protocol_refusals() -> Result<(), Error> {
    use std::os::unix::process::ExitStatusExt;
    let output = |code| Output {
        status: ExitStatus::from_raw(code << 8),
        stdout: Vec::new(),
        stderr: b"diagnostic".to_vec(),
    };
    assert!(batch_support(&output(0))?);
    assert!(!batch_support(&output(129))?);
    assert!(batch_support(&output(128)).is_err());
    assert!(batch_support(&output(1)).is_err());
    let identity = "1234567890123456789012345678901234567890";
    let mut valid = format!("{identity} blob 4\0").into_bytes();
    valid.extend_from_slice(b"\0\n\xffx\0");
    assert_eq!(read_object(&mut Cursor::new(valid), "pin:path")?.bytes, b"\0\n\xffx");
    for frame in [
        b"pin:path missing\0".to_vec(),
        b"pin:path missing\n".to_vec(),
        b"bad blob 0\0\0".to_vec(),
        format!("{identity} unknown 0\0\0").into_bytes(),
        format!("{identity} blob invalid\0").into_bytes(),
        format!("{identity} blob 2\0x").into_bytes(),
        format!("{identity} blob 1\0xy").into_bytes(),
        format!("{identity} blob 0 extra\0\0").into_bytes(),
    ] {
        assert!(read_object(&mut Cursor::new(frame), "pin:path").is_err());
    }
    Ok(())
}
