//! Native descriptor primitives behind the public `std.fs` publication contract.
//!
//! The Incan stdlib owns the publication recipe and user-visible errors. This module only keeps host file descriptors
//! alive for directory synchronization and OS-backed advisory locks.

use std::fs::File;
#[cfg(unix)]
use std::fs::OpenOptions;
use std::io;
#[cfg(unix)]
use std::path::{Path, PathBuf};

#[cfg(unix)]
use rustix::fs::{FlockOperation, flock};

/// An open lock-file descriptor. Dropping it releases the OS advisory lock.
#[derive(Debug)]
pub struct RawFileLock {
    _file: File,
}

#[cfg(unix)]
/// Derive the stable sibling lock-file location for one protected logical path.
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

#[cfg(unix)]
/// Open or create the persistent sibling file that owns an advisory lock identity.
fn open_lock(path: &str) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path(path)?)
}

#[cfg(unix)]
/// Acquire the requested host advisory lock and retain its owning descriptor.
fn lock(path: &str, operation: FlockOperation) -> io::Result<RawFileLock> {
    let file = open_lock(path)?;
    flock(&file, operation)?;
    Ok(RawFileLock { _file: file })
}

/// Request persistence of directory-entry updates for `path`.
#[cfg(unix)]
pub fn sync_directory(path: String) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| error.to_string())
}

/// Report that directory synchronization is not available on this host.
#[cfg(not(unix))]
pub fn sync_directory(_path: String) -> Result<(), String> {
    Err("unsupported: directory synchronization requires a Unix host".to_string())
}

/// Acquire a blocking shared advisory lock for the supplied logical path.
#[cfg(unix)]
pub fn raw_lock_shared(path: String) -> Result<RawFileLock, String> {
    lock(&path, FlockOperation::LockShared).map_err(|error| error.to_string())
}

/// Acquire a blocking exclusive advisory lock for the supplied logical path.
#[cfg(unix)]
pub fn raw_lock_exclusive(path: String) -> Result<RawFileLock, String> {
    lock(&path, FlockOperation::LockExclusive).map_err(|error| error.to_string())
}

/// Attempt to acquire a shared advisory lock without blocking.
#[cfg(unix)]
pub fn raw_try_lock_shared(path: String) -> Result<Option<RawFileLock>, String> {
    match lock(&path, FlockOperation::NonBlockingLockShared) {
        Ok(lock) => Ok(Some(lock)),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

/// Attempt to acquire an exclusive advisory lock without blocking.
#[cfg(unix)]
pub fn raw_try_lock_exclusive(path: String) -> Result<Option<RawFileLock>, String> {
    match lock(&path, FlockOperation::NonBlockingLockExclusive) {
        Ok(lock) => Ok(Some(lock)),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

macro_rules! unsupported_lock_api {
    ($name:ident, $return:ty) => {
        #[cfg(not(unix))]
        pub fn $name(_path: String) -> Result<$return, String> {
            Err("unsupported: advisory file locks require a Unix host".to_string())
        }
    };
}

unsupported_lock_api!(raw_lock_shared, RawFileLock);
unsupported_lock_api!(raw_lock_exclusive, RawFileLock);
unsupported_lock_api!(raw_try_lock_shared, Option<RawFileLock>);
unsupported_lock_api!(raw_try_lock_exclusive, Option<RawFileLock>);

#[cfg(all(test, unix))]
mod tests {
    use super::{raw_lock_exclusive, raw_lock_shared, raw_try_lock_exclusive, raw_try_lock_shared};
    use std::env;
    use std::fs;
    use std::io;
    use std::process::Command;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    const LOCK_TARGET_ENV: &str = "INCAN_FS_LOCK_TEST_TARGET";
    const LOCK_READY_ENV: &str = "INCAN_FS_LOCK_TEST_READY";
    const LOCK_MODE_ENV: &str = "INCAN_FS_LOCK_TEST_MODE";

    /// Produce a collision-resistant temporary test-directory suffix.
    fn timestamp_suffix() -> Result<u128, io::Error> {
        Ok(SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos())
    }

    #[test]
    #[ignore = "helper process for exclusive_lock_rejects_other_process"]
    /// Holds a requested lock mode until a parent conformance test observes its state.
    fn lock_holder() -> Result<(), Box<dyn std::error::Error>> {
        let target = env::var(LOCK_TARGET_ENV)?;
        let ready = env::var(LOCK_READY_ENV)?;
        let mode = match env::var(LOCK_MODE_ENV) {
            Ok(mode) => mode,
            Err(_) => "exclusive".to_string(),
        };
        let _lock = if mode == "shared" {
            raw_lock_shared(target).map_err(io::Error::other)?
        } else {
            raw_lock_exclusive(target).map_err(io::Error::other)?
        };
        fs::write(ready, "ready")?;
        thread::sleep(Duration::from_secs(30));
        Ok(())
    }

    #[test]
    /// Confirms independent shared holders coexist while a conflicting exclusive holder is rejected.
    fn shared_locks_coexist_but_block_exclusive_claimants() -> Result<(), Box<dyn std::error::Error>> {
        let root = env::temp_dir().join(format!(
            "incan_stdlib_fs_shared_lock_{}_{}",
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
            .env(LOCK_MODE_ENV, "shared")
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
            return Err("shared lock-holder child did not signal readiness".into());
        }

        let second_shared = raw_try_lock_shared(target.to_string_lossy().into_owned()).map_err(io::Error::other)?;
        assert!(
            second_shared.is_some(),
            "shared locks from separate processes must coexist"
        );
        drop(second_shared);
        let exclusive = raw_try_lock_exclusive(target.to_string_lossy().into_owned()).map_err(io::Error::other)?;
        assert!(
            exclusive.is_none(),
            "a held shared lock must block an exclusive claimant"
        );

        child.kill()?;
        child.wait()?;
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    /// Confirms an exclusive lock held by another process rejects a second exclusive claim.
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

    #[test]
    /// Ensures concurrent readers observe only complete old or new payloads during repeated replacement.
    fn atomic_replace_readers_observe_complete_payloads() -> Result<(), Box<dyn std::error::Error>> {
        let root = env::temp_dir().join(format!(
            "incan_stdlib_fs_replace_{}_{}",
            std::process::id(),
            timestamp_suffix()?
        ));
        fs::create_dir_all(&root)?;
        let target = root.join("published");
        let staged = root.join("staged");
        let old_payload = vec![b'a'; 16 * 1024];
        let new_payload = vec![b'b'; 16 * 1024];
        fs::write(&target, &old_payload)?;

        let reading = Arc::new(AtomicBool::new(true));
        let reader_started = Arc::new(AtomicBool::new(false));
        let reader_target = target.clone();
        let reader_old = old_payload.clone();
        let reader_new = new_payload.clone();
        let reader_reading = Arc::clone(&reading);
        let reader_started_signal = Arc::clone(&reader_started);
        let reader = thread::spawn(move || -> Result<(), io::Error> {
            reader_started_signal.store(true, Ordering::Release);
            while reader_reading.load(Ordering::Acquire) {
                let observed = fs::read(&reader_target)?;
                if observed != reader_old && observed != reader_new {
                    return Err(io::Error::other("reader observed a partial replacement payload"));
                }
            }
            Ok(())
        });

        while !reader_started.load(Ordering::Acquire) {
            thread::yield_now();
        }
        for index in 0..500 {
            let next = if index % 2 == 0 { &new_payload } else { &old_payload };
            fs::write(&staged, next)?;
            fs::rename(&staged, &target)?;
            thread::yield_now();
        }
        reading.store(false, Ordering::Release);
        reader
            .join()
            .map_err(|_| io::Error::other("replacement reader thread panicked"))??;
        fs::remove_dir_all(root)?;
        Ok(())
    }
}
