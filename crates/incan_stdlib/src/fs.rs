//! Native descriptor primitives behind the public `std.fs` publication contract.
//!
//! The Incan stdlib owns the publication recipe and user-visible errors. This module only keeps host file descriptors
//! alive for directory synchronization and OS-backed advisory locks.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use rustix::fs::{FlockOperation, flock};

/// An open lock-file descriptor. Dropping it releases the OS advisory lock.
#[derive(Debug)]
pub struct RawFileLock {
    _file: File,
}

fn lock_path(path: &str) -> io::Result<PathBuf> {
    let protected = Path::new(path);
    let parent = protected.parent().unwrap_or_else(|| Path::new("."));
    let name = protected.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "file locks require a path with a final component",
        )
    })?;
    let lock_name = format!(".{}.incan.lock", name.to_string_lossy());
    Ok(parent.join(lock_name))
}

fn open_lock(path: &str) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path(path)?)
}

fn lock(path: &str, operation: FlockOperation) -> io::Result<RawFileLock> {
    let file = open_lock(path)?;
    flock(&file, operation)?;
    Ok(RawFileLock { _file: file })
}

/// Request persistence of directory-entry updates for `path`.
pub fn sync_directory(path: String) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| error.to_string())
}

/// Acquire a blocking shared advisory lock for the supplied logical path.
pub fn raw_lock_shared(path: String) -> Result<RawFileLock, String> {
    lock(&path, FlockOperation::LockShared).map_err(|error| error.to_string())
}

/// Acquire a blocking exclusive advisory lock for the supplied logical path.
pub fn raw_lock_exclusive(path: String) -> Result<RawFileLock, String> {
    lock(&path, FlockOperation::LockExclusive).map_err(|error| error.to_string())
}

/// Attempt to acquire a shared advisory lock without blocking.
pub fn raw_try_lock_shared(path: String) -> Result<Option<RawFileLock>, String> {
    match lock(&path, FlockOperation::NonBlockingLockShared) {
        Ok(lock) => Ok(Some(lock)),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

/// Attempt to acquire an exclusive advisory lock without blocking.
pub fn raw_try_lock_exclusive(path: String) -> Result<Option<RawFileLock>, String> {
    match lock(&path, FlockOperation::NonBlockingLockExclusive) {
        Ok(lock) => Ok(Some(lock)),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::{raw_lock_exclusive, raw_try_lock_exclusive};
    use std::env;
    use std::fs;
    use std::io;
    use std::process::Command;
    use std::thread;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    const LOCK_TARGET_ENV: &str = "INCAN_FS_LOCK_TEST_TARGET";
    const LOCK_READY_ENV: &str = "INCAN_FS_LOCK_TEST_READY";

    fn timestamp_suffix() -> Result<u128, io::Error> {
        Ok(SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos())
    }

    #[test]
    #[ignore = "helper process for exclusive_lock_rejects_other_process"]
    fn lock_holder() -> Result<(), Box<dyn std::error::Error>> {
        let target = env::var(LOCK_TARGET_ENV)?;
        let ready = env::var(LOCK_READY_ENV)?;
        let _lock = raw_lock_exclusive(target).map_err(io::Error::other)?;
        fs::write(ready, "ready")?;
        thread::sleep(Duration::from_secs(30));
        Ok(())
    }

    #[test]
    fn exclusive_lock_rejects_other_process() -> Result<(), Box<dyn std::error::Error>> {
        let root = env::temp_dir().join(format!(
            "incan_stdlib_fs_lock_{}_{}",
            std::process::id(),
            timestamp_suffix()?
        ));
        fs::create_dir_all(&root)?;
        let target = root.join("state.lock");
        let ready = root.join("ready");
        let executable = env::current_exe()?;
        let mut child = Command::new(executable)
            .args(["--exact", "fs::tests::lock_holder", "--ignored", "--nocapture"])
            .env(LOCK_TARGET_ENV, target.as_os_str())
            .env(LOCK_READY_ENV, ready.as_os_str())
            .spawn()?;

        let mut ready_seen = false;
        for _ in 0..200 {
            if ready.exists() {
                ready_seen = true;
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        if !ready_seen {
            let _ = child.kill();
            let _ = child.wait();
            fs::remove_dir_all(&root)?;
            return Err("lock-holder child did not signal readiness".into());
        }

        let contending = raw_try_lock_exclusive(target.to_string_lossy().into_owned()).map_err(io::Error::other)?;
        assert!(
            contending.is_none(),
            "a second process must observe the held exclusive lock"
        );

        child.kill()?;
        child.wait()?;
        fs::remove_dir_all(root)?;
        Ok(())
    }
}
